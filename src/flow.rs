// flow.rs
//
// Copyright (C) 2019-2020  Minnesota Department of Transportation
//
use crate::error::Error;
use glib::{Cast, ObjectExt, ToSendValue, ToValue, WeakRef};
use gstreamer::{
    Bus, Caps, ClockTime, Element, ElementExt, ElementExtManual,
    ElementFactory, GObjectExtManualGst, GstBinExt, GstObjectExt, Message,
    MessageView, Pad, PadExt, PadExtManual, Pipeline, Sample, State, Structure,
};
use gstreamer_video::{VideoOverlay, VideoOverlayExtManual};
use log::{debug, error, trace, warn};
use std::convert::TryFrom;
use std::fmt;
use std::str::FromStr;
use std::sync::mpsc::Sender;

/// One second (microsecond units)
const SEC_US: u64 = 1_000_000;

/// One second (nanosecond units)
const SEC_NS: u64 = 1_000_000_000;

/// RTSP stream number for video
const STREAM_NUM_VIDEO: u32 = 0;

/// Default source timeout (sec)
const DEFAULT_TIMEOUT_SEC: u16 = 2;

/// Default buffering latency (ms)
const DEFAULT_LATENCY_MS: u32 = 100;

/// Time-To-Live for multicast packets
const TTL_MULTICAST: i32 = 15;

/// Clock rate for RTP video packets
const RTP_VIDEO_CLOCK_RATE: i32 = 90_000;

/// Number of times to check PTS before giving up
const PTS_CHECK_TRIES: usize = 5;

/// Font size (pt), using default height
const FONT_SZ: u32 = 14;

/// Text overlay color (ARGB; yellowish white)
const OVERLAY_COLOR: u32 = 0xFF_FF_FF_E0;

/// Default height (px)
const DEFAULT_HEIGHT: u32 = 240;

/// User agent including version
const AGENT: &'static str = concat!("streambed/", env!("CARGO_PKG_VERSION"));

/// Network transport
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transport {
    /// Any transport
    ANY,
    /// UDP transport
    UDP,
    /// UDP multicast transport
    MCAST,
    /// TCP transport
    TCP,
}

/// Video encoding
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    /// Raw video
    RAW,
    /// Portable Network Graphics
    PNG,
    /// Motion JPEG
    MJPEG,
    /// MPEG-2 TS
    MPEG2,
    /// MPEG-4
    MPEG4,
    /// H.264
    H264,
    /// H.265
    H265,
    /// VP8 video
    VP8,
    /// VP9 video
    VP9,
    /// AV1 video
    AV1,
}

/// Video source
pub struct Source {
    /// Source location URI
    location: String,
    /// RTSP transport
    rtsp_transport: Transport,
    /// Source encoding
    encoding: Encoding,
    /// RTP source properties (from SDP)
    sprops: Option<String>,
    /// Source timeout (sec)
    timeout: u16,
    /// Buffering latency (ms)
    latency: u32,
}

/// Pixel aspect ratio handling
#[derive(Clone, Copy)]
pub enum AspectRatio {
    /// Adjust pixel aspect ratio to fill sink window
    FILL,
    /// Preserve pixel aspect ratio
    PRESERVE,
}

/// Video matrix crop configuration
#[derive(Clone, Copy)]
pub struct MatrixCrop {
    /// Aspect ratio setting
    aspect: AspectRatio,
    /// X-position `0..=7` in matrix
    x: u8,
    /// Y-position `0..=7` in matrix
    y: u8,
    /// Width `1..=8` of matrix
    width: u8,
    /// Height `1..=8` of matrix
    height: u8,
    /// Horizontal gap at edges
    ///
    /// Value in hundredths of percent of window `0.00 to 100.00`
    hgap: u32,
    /// Vertical gap at edges
    ///
    /// Value in hundredths of percent of window `0.00 to 100.00`
    vgap: u32,
}

/// Video sink
pub enum Sink {
    /// Fake sink (for testing)
    FAKE,
    /// RTP over UDP (addr, port, encoding, insert_config)
    RTP(String, i32, Encoding, bool),
    /// Window sink
    WINDOW(MatrixCrop),
}

/// Flow feedback
pub enum Feedback {
    /// Flow playing
    Playing(usize),
    /// Flow stopped
    Stopped(usize),
    /// Update statistics
    Stats(usize, u64, u64, u64),
}

impl fmt::Display for Feedback {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Feedback::Playing(idx) => write!(f, "Flow{} playing", idx),
            Feedback::Stopped(idx) => write!(f, "Flow{} stopped", idx),
            Feedback::Stats(idx, pushed, lost, late) => {
                write!(f, "Flow{} stats: {} pushed, {} lost, {} late",
                    idx, pushed, lost, late)
            }
        }
    }
}

/// Hardware video acceleration
#[derive(Clone, Copy)]
pub enum Acceleration {
    /// No video acceleration
    NONE,
    /// Video Acceleration API
    VAAPI,
    /// OpenMAX
    OMX,
}

/// Builder for video flows
///
/// After a Flow is built, these structs are owned
/// by a bus watcher which handles bus messages.
#[derive(Default)]
pub struct FlowBuilder {
    /// Index of flow
    idx: usize,
    /// Video source config
    source: Source,
    /// Video sink config
    sink: Sink,
    /// Hardware acceleration
    acceleration: Acceleration,
    /// Overlay text
    overlay_text: Option<String>,
    /// Flow feedback
    feedback: Option<Sender<Feedback>>,
    /// Video overlay handle
    handle: Option<usize>,
    /// Pipeline for flow
    pipeline: WeakRef<Pipeline>,
    /// Head element of pipeline
    head: Option<Element>,
    /// Flag indicating playing
    is_playing: bool,
    /// Number of pushed packets
    pushed: u64,
    /// Number of lost packets
    lost: u64,
    /// Number of late packets
    late: u64,
}

/// Video flow
pub struct Flow {
    /// Video pipeline
    pipeline: Pipeline,
    /// Pipeline message bus
    bus: Bus,
}

/// Periodic flow checker
struct FlowChecker {
    /// Index of flow
    idx: usize,
    /// Flow pipeline
    pipeline: WeakRef<Pipeline>,
    /// Count of checks
    count: usize,
    /// Most recent presentation time stamp
    last_pts: ClockTime,
}

/// Make a pipeline element
fn make_element(
    factory_name: &'static str, name: Option<&str>,
) -> Result<Element, Error> {
    ElementFactory::make(factory_name, name).map_err(|_| {
        error!("make_element: {}", factory_name);
        Error::MissingElement(factory_name)
    })
}

/// Set a property of an element
fn set_property(
    elem: &Element, name: &'static str, value: &dyn ToValue,
) -> Result<(), Error> {
    match elem.set_property(name, value) {
        Ok(()) => Ok(()),
        Err(_) => Err(Error::InvalidProperty(name)),
    }
}

/// Link ghost pad with sink
fn link_ghost_pad(src: &Element, src_pad: &Pad, sink: Element) {
    match sink.get_static_pad("sink") {
        Some(sink_pad) => {
            let pn = src_pad.get_name();
            let p0 = src.get_name();
            let p1 = sink.get_name();
            match src_pad.link(&sink_pad) {
                Ok(_) => debug!("pad {} linked: {} => {}", pn, p0, p1),
                Err(_) => debug!("pad {} not linked: {} => {}", pn, p0, p1),
            }
        }
        None => error!("no sink pad"),
    }
}

impl Default for AspectRatio {
    fn default() -> Self {
        AspectRatio::PRESERVE
    }
}

impl Default for Acceleration {
    fn default() -> Self {
        Acceleration::NONE
    }
}

impl FromStr for Acceleration {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "" | "NONE" => Ok(Self::NONE),
            "VAAPI" => Ok(Self::VAAPI),
            "OMX" => Ok(Self::OMX),
            _ => Err(Error::Other("invalid acceleration")),
        }
    }
}

impl Default for Transport {
    fn default() -> Self {
        Transport::ANY
    }
}

impl FromStr for Transport {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ANY" => Ok(Self::ANY),
            "UDP" => Ok(Self::UDP),
            "MCAST" => Ok(Self::MCAST),
            "TCP" => Ok(Self::TCP),
            _ => Err(Error::Other("invalid transport")),
        }
    }
}

impl Default for Encoding {
    fn default() -> Self {
        Encoding::RAW
    }
}

impl FromStr for Encoding {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RAW" => Ok(Self::RAW),
            "PNG" => Ok(Self::PNG),
            "MJPEG" => Ok(Self::MJPEG),
            "MPEG2" => Ok(Self::MPEG2),
            "MPEG4" => Ok(Self::MPEG4),
            "H264" | "" => Ok(Self::H264),
            "H265" => Ok(Self::H265),
            "VP8" => Ok(Self::VP8),
            "VP9" => Ok(Self::VP9),
            _ => Err(Error::Other("invalid encoding")),
        }
    }
}

impl Encoding {
    /// Get RTP depayload factory name
    fn rtp_depay(&self) -> Result<&'static str, Error> {
        match self {
            Encoding::RAW => Ok("rtpvrawdepay"),
            Encoding::MJPEG => Ok("rtpjpegdepay"),
            Encoding::MPEG2 => Ok("rtpmp2tdepay"),
            Encoding::MPEG4 => Ok("rtpmp4vdepay"),
            Encoding::H264 => Ok("rtph264depay"),
            Encoding::H265 => Ok("rtph265depay"),
            Encoding::VP8 => Ok("rtpvp8depay"),
            Encoding::VP9 => Ok("rtpvp9depay"),
            _ => Err(Error::Other("invalid encoding for RTP")),
        }
    }
    /// Get RTP payload factory name
    fn rtp_pay(&self) -> Result<&'static str, Error> {
        match self {
            Encoding::RAW => Ok("rtpvrawpay"),
            Encoding::MJPEG => Ok("rtpjpegpay"),
            Encoding::MPEG2 => Ok("rtpmp2tpay"),
            Encoding::MPEG4 => Ok("rtpmp4vpay"),
            Encoding::H264 => Ok("rtph264pay"),
            Encoding::H265 => Ok("rtph265pay"),
            Encoding::VP8 => Ok("rtpvp8pay"),
            Encoding::VP9 => Ok("rtpvp9pay"),
            _ => Err(Error::Other("invalid encoding for RTP")),
        }
    }
}

impl Default for Source {
    fn default() -> Self {
        Source {
            location: String::new(),
            rtsp_transport: Transport::default(),
            encoding: Encoding::default(),
            sprops: None,
            timeout: DEFAULT_TIMEOUT_SEC,
            latency: DEFAULT_LATENCY_MS,
        }
    }
}

impl Source {
    /// Use the specified location
    pub fn with_location(mut self, location: &str) -> Self {
        self.location = location.to_string();
        self
    }

    /// Use the specified transport
    pub fn with_rtsp_transport(mut self, rtsp_transport: Transport) -> Self {
        self.rtsp_transport = rtsp_transport;
        self
    }

    /// Use the specified encoding
    pub fn with_encoding(mut self, encoding: Encoding) -> Self {
        self.encoding = encoding;
        self
    }

    /// Use the specified SDP properties
    pub fn with_sprops(mut self, sprops: Option<&str>) -> Self {
        self.sprops = sprops.map(|s| s.to_string());
        self
    }

    /// Use the specified timeout (sec)
    pub fn with_timeout(mut self, timeout: u16) -> Self {
        self.timeout = timeout;
        self
    }

    /// Use the specified buffering latency (ms)
    pub fn with_latency(mut self, latency: u32) -> Self {
        self.latency = latency;
        self
    }

    /// Get timeout as seconds
    fn timeout_s(&self) -> u32 {
        u32::from(self.timeout)
    }

    /// Get timeout as milliseconds
    fn timeout_ms(&self) -> u32 {
        u32::from(self.timeout) * 1_000
    }

    /// Get timeout as microseconds
    fn timeout_us(&self) -> u64 {
        u64::from(self.timeout) * SEC_US
    }

    /// Get timeout as nanoseconds
    fn timeout_ns(&self) -> u64 {
        u64::from(self.timeout) * SEC_NS
    }

    /// Check if source is RTP
    fn is_rtp(&self) -> bool {
        self.location.starts_with("udp://")
    }

    /// Check if source is RTSP
    fn is_rtsp(&self) -> bool {
        self.location.starts_with("rtsp://")
    }

    /// Check if source is RTP or RTSP
    fn is_rtp_or_rtsp(&self) -> bool {
        self.is_rtp() || self.is_rtsp()
    }

    /// Check if source is HTTP
    fn is_http(&self) -> bool {
        self.location.starts_with("http://")
    }
}

impl AspectRatio {
    /// Get as boolean value
    fn as_bool(&self) -> bool {
        match self {
            AspectRatio::FILL => false,
            AspectRatio::PRESERVE => true,
        }
    }
}

impl Default for MatrixCrop {
    fn default() -> Self {
        MatrixCrop {
            aspect: AspectRatio::default(),
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            hgap: 0,
            vgap: 0,
        }
    }
}

impl TryFrom<&str> for MatrixCrop {
    type Error = Error;

    fn try_from(v: &str) -> Result<Self, Self::Error> {
        let p: Vec<_> = v.split(',').collect();
        if p.len() == 3 {
            let mut crop = p[0].chars();
            let x = MatrixCrop::position(crop.next().unwrap_or_default())?;
            let y = MatrixCrop::position(crop.next().unwrap_or_default())?;
            let width =
                MatrixCrop::position(crop.next().unwrap_or_default())? + 1;
            let height =
                MatrixCrop::position(crop.next().unwrap_or_default())? + 1;
            if x < width && y < height {
                let hgap: u32 = p[1].parse()?;
                let vgap: u32 = p[2].parse()?;
                // Don't allow more than 50% gap
                if hgap <= MatrixCrop::PERCENT / 2
                    && vgap <= MatrixCrop::PERCENT / 2
                {
                    return Ok(MatrixCrop {
                        aspect: AspectRatio::default(),
                        x,
                        y,
                        width,
                        height,
                        hgap,
                        vgap,
                    });
                }
            }
        }
        Err(Error::InvalidCrop())
    }
}

impl MatrixCrop {
    /// Percent of window, in hundredths
    const PERCENT: u32 = 100_00;

    /// Get a matrix position from a crop code
    fn position(c: char) -> Result<u8, Error> {
        match c {
            'A'..='H' => Ok(c as u8 - b'A'),
            _ => Err(Error::InvalidCrop()),
        }
    }

    /// Check if window is cropped
    fn is_cropped(&self) -> bool {
        self.width > 1 || self.height > 1
    }

    /// Get number of pixels to crop from top edge
    fn top(&self, height: u32) -> u32 {
        let num = u32::from(self.y);
        let den = u32::from(self.height);
        let pix = height * num / den;
        let gap = height * self.vgap / (den * MatrixCrop::PERCENT * 2);
        debug!("crop top: {} + {} = {}", pix, gap, pix + gap);
        pix + gap
    }

    /// Get number of pixels to crop from bottom edge
    fn bottom(&self, height: u32) -> u32 {
        let num = u32::from(self.height - self.y - 1);
        let den = u32::from(self.height);
        let pix = height * num / den;
        let gap = height * self.vgap / (den * MatrixCrop::PERCENT * 2);
        debug!("crop bottom: {} + {} = {}", pix, gap, pix + gap);
        pix + gap
    }

    /// Get number of pixels to crop from left edge
    fn left(&self, width: u32) -> u32 {
        let num = u32::from(self.x);
        let den = u32::from(self.width);
        let pix = width * num / den;
        let gap = width * self.hgap / (den * MatrixCrop::PERCENT * 2);
        debug!("crop left: {} + {} = {}", pix, gap, pix + gap);
        pix + gap
    }

    /// Get number of pixels to crop from right edge
    fn right(&self, width: u32) -> u32 {
        let num = u32::from(self.width - self.x - 1);
        let den = u32::from(self.width);
        let pix = width * num / den;
        let gap = width * self.hgap / (den * MatrixCrop::PERCENT * 2);
        debug!("crop right: {} + {} = {}", pix, gap, pix + gap);
        pix + gap
    }
}

impl Default for Sink {
    fn default() -> Self {
        Sink::FAKE
    }
}

impl Sink {
    /// Is the sink RTP?
    fn is_rtp(&self) -> bool {
        match self {
            Sink::RTP(_, _, _, _) => true,
            _ => false,
        }
    }
    /// Get the gstreamer factory name
    fn factory_name(&self, acceleration: Acceleration) -> &'static str {
        match (self, acceleration) {
            (Sink::FAKE, _) => "fakesink",
            (Sink::RTP(_, _, _, _), _) => "udpsink",
            (Sink::WINDOW(_), Acceleration::VAAPI) => "vaapisink",
            (Sink::WINDOW(_), _) => "gtksink",
        }
    }
    /// Get the matrix crop setting
    fn crop(&self) -> MatrixCrop {
        match self {
            Sink::WINDOW(c) => *c,
            _ => MatrixCrop::default(),
        }
    }
    /// Get the sink encoding
    fn encoding(&self) -> Encoding {
        match self {
            Sink::RTP(_, _, encoding, _) => *encoding,
            _ => Encoding::RAW,
        }
    }
    /// Should config be inserted in-band?
    fn insert_config(&self) -> bool {
        match self {
            Sink::RTP(_, _, _, true) => true,
            _ => false,
        }
    }
}

impl fmt::Display for FlowBuilder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Flow{}", self.idx)
    }
}

impl FlowBuilder {
    /// Create a new flow builder
    pub fn new(idx: usize) -> Self {
        FlowBuilder {
            idx,
            ..Default::default()
        }
    }

    /// Use the specified source
    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    /// Use the specified sink
    pub fn with_sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Use the specified video acceleration
    pub fn with_acceleration(mut self, acceleration: Acceleration) -> Self {
        self.acceleration = acceleration;
        self
    }

    /// Use the specified overlay text
    pub fn with_overlay_text(mut self, overlay_text: Option<&str>) -> Self {
        self.overlay_text = overlay_text.map(|t| t.to_string());
        self
    }

    /// Use the specified flow feedback
    pub fn with_feedback(
        mut self, feedback: Option<Sender<Feedback>>,
    ) -> Self {
        self.feedback = feedback;
        self
    }

    /// Use the specified video overlay window handle
    pub fn with_handle(mut self, handle: Option<usize>) -> Self {
        self.handle = handle;
        self
    }

    /// Build the flow
    pub fn build(mut self) -> Result<Flow, Error> {
        let idx = self.idx;
        let name = format!("m{}", self.idx);
        let pipeline = Pipeline::new(Some(&name));
        self.pipeline = pipeline.downgrade();
        self.add_elements()?;
        self.is_playing = true;
        self.stopped();
        let timeout_ms = self.source.timeout_ms();
        let bus = pipeline.get_bus().unwrap();
        if let Err(_) = bus.add_watch(move |_bus, m| self.handle_message(m)) {
            return Err(Error::ConnectSignal("watch"));
        }
        let mut checker = FlowChecker::new(idx, pipeline.downgrade());
        glib::source::timeout_add(timeout_ms, move || checker.do_check());
        pipeline.set_state(State::Playing).unwrap();
        Ok(Flow {
            pipeline,
            bus,
        })
    }

    /// Check if pipeline should have a text overlay
    fn has_text(&self) -> bool {
        self.overlay_text.is_some()
    }

    /// Add all required elements to the pipeline
    ///
    /// Pipeline is built from sink to source.
    fn add_elements(&mut self) -> Result<(), Error> {
        self.add_element(self.create_sink()?)?;
        if self.needs_rtp_pay() {
            self.add_rtp_pay()?;
        }
        if self.needs_encode() {
            self.add_encode()?;
            self.add_queue()?;
        }
        if self.sink.crop().is_cropped() {
            self.add_element(make_element("videobox", Some("vbox"))?)?;
        }
        if self.has_text() {
            self.add_element(self.create_text()?)?;
            self.add_queue()?;
        }
        if self.needs_decode() {
            self.add_decode()?;
            self.add_queue()?;
        }
        if self.needs_rtp_depay() {
            let depay = make_element(self.source.encoding.rtp_depay()?, None)?;
            self.add_element(depay)?;
        }
        if self.is_rtp_passthru() {
            self.add_queue()?;
        }
        self.add_source()?;
        self.head = None;
        Ok(())
    }

    /// Check if pipeline needs RTP payloader
    fn needs_rtp_pay(&self) -> bool {
        self.sink.is_rtp() && !self.is_rtp_passthru()
    }

    /// Check if pipeline needs RTP depayloader
    fn needs_rtp_depay(&self) -> bool {
        self.source.is_rtp_or_rtsp() && !self.is_rtp_passthru()
    }

    /// Check if RTP can pass unchanged from source to sink
    fn is_rtp_passthru(&self) -> bool {
        self.source.is_rtp_or_rtsp()
            && self.sink.is_rtp()
            && !self.sink.insert_config()
            && !self.needs_transcode()
    }

    /// Check if pipeline needs transcoding
    fn needs_transcode(&self) -> bool {
        self.source.encoding != self.sink.encoding() || self.has_text()
    }

    /// Check if pipeline needs encoding
    fn needs_encode(&self) -> bool {
        self.sink.encoding() != Encoding::RAW && self.needs_transcode()
    }

    /// Check if pipeline needs decoding
    fn needs_decode(&self) -> bool {
        self.source.encoding != Encoding::RAW && self.needs_transcode()
    }

    /// Add RTP payload element
    fn add_rtp_pay(&mut self) -> Result<(), Error> {
        let pay = make_element(self.sink.encoding().rtp_pay()?, None)?;
        if self.sink.insert_config() {
            match self.sink.encoding() {
                Encoding::MPEG4 => {
                    // send configuration headers once per second
                    set_property(&pay, "config-interval", &1u32)?;
                }
                Encoding::H264 | Encoding::H265 => {
                    // send sprop parameter sets every IDR frame (-1)
                    set_property(&pay, "config-interval", &(-1))?;
                }
                _ => (),
            }
        }
        self.add_element(pay)
    }

    /// Add encode elements
    fn add_encode(&mut self) -> Result<(), Error> {
        match self.sink.encoding() {
            Encoding::RAW => Ok(()),
            Encoding::MJPEG => self.add_element(make_element("jpegenc", None)?),
            Encoding::MPEG2 => {
                self.add_element(make_element("mpegtsmux", None)?)?;
                self.add_element(make_element("mpeg2enc", None)?)
            }
            Encoding::MPEG4 => self.add_element(self.create_mpeg4enc()?),
            Encoding::H264 => self.add_element(self.create_h264enc()?),
            Encoding::H265 => self.add_element(self.create_h265enc()?),
            Encoding::VP8 => self.add_element(self.create_vp8enc()?),
            Encoding::VP9 => self.add_element(self.create_vp9enc()?),
            Encoding::AV1 => self.add_element(make_element("av1enc", None)?),
            _ => Err(Error::Other("invalid encoding")),
        }
    }

    /// Create MPEG-4 encode element
    fn create_mpeg4enc(&self) -> Result<Element, Error> {
        make_element("avenc_mpeg4", None)
    }

    /// Create h.264 encode element
    fn create_h264enc(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => {
                let enc = make_element("vaapih264enc", None)?;
                // Quality-level ranges to 1 (best) to 7 (worst)
                set_property(&enc, "quality-level", &6u32)?;
                enc.set_property_from_str("tune", &"low-power");
                Ok(enc)
            }
            Acceleration::OMX => make_element("omxh264enc", None),
            _ => {
                let enc = make_element("x264enc", None)?;
                enc.set_property_from_str("tune", &"zerolatency");
                // With the default "medium" speed-preset, the pipeline can't
                // run live.  With "superfast", the quality is still very good.
                // ultrafast (1), superfast (2), veryfast (3), faster (4),
                // fast (5), medium (6), etc.
                enc.set_property_from_str("speed-preset", &"superfast");
                Ok(enc)
            }
        }
    }

    /// Create h.265 encode element
    fn create_h265enc(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => {
                let enc = make_element("vaapih265enc", None)?;
                // Quality-level ranges to 1 (best) to 7 (worst)
                set_property(&enc, "quality-level", &6u32)?;
                enc.set_property_from_str("tune", &"low-power");
                Ok(enc)
            }
            _ => {
                let enc = make_element("x265enc", None)?;
                enc.set_property_from_str("tune", &"zerolatency");
                enc.set_property_from_str("speed-preset", &"superfast");
                Ok(enc)
            }
        }
    }

    /// Create VP8 encode element
    fn create_vp8enc(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => make_element("vaapivp8enc", None),
            _ => make_element("vp8enc", None),
        }
    }

    /// Create VP9 encode element
    fn create_vp9enc(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => make_element("vaapivp9enc", None),
            _ => make_element("vp9enc", None),
        }
    }

    /// Add source elements
    fn add_source(&mut self) -> Result<(), Error> {
        if self.source.is_rtp() {
            self.add_source_rtp()
        } else if self.source.is_rtsp() {
            self.add_source_rtsp()
        } else if self.source.is_http() {
            self.add_source_http()
        } else {
            self.add_source_test()
        }
    }

    /// Add source elements for an RTP flow
    fn add_source_rtp(&mut self) -> Result<(), Error> {
        if !self.source.is_rtsp() {
            let jtr = make_element("rtpjitterbuffer", Some("jitter"))?;
            set_property(&jtr, "latency", &self.source.latency)?;
            set_property(&jtr, "max-dropout-time", &self.source.timeout_ms())?;
            self.add_element(jtr)?;
            let fltr = make_element("capsfilter", None)?;
            let caps = self.create_rtp_caps()?;
            set_property(&fltr, "caps", &caps)?;
            self.add_element(fltr)?;
        }
        let src = make_element("udpsrc", None)?;
        set_property(&src, "uri", &self.source.location)?;
        // Post GstUDPSrcTimeout messages after timeout (0 for disabled)
        set_property(&src, "timeout", &self.source.timeout_ns())?;
        self.add_element(src)
    }

    /// Create RTP caps for filter element
    fn create_rtp_caps(&self) -> Result<Caps, Error> {
        let mut values: Vec<(&str, &dyn ToSendValue)> =
            vec![("clock-rate", &RTP_VIDEO_CLOCK_RATE)];
        if let Encoding::MPEG2 = self.source.encoding {
            values.push(("encoding-name", &"MP2T"));
        }
        if let Some(sprops) = &self.source.sprops {
            values.push(("sprop-parameter-sets", &sprops));
            return Ok(Caps::new_simple("application/x-rtp", &values[..]));
        }
        Ok(Caps::new_simple("application/x-rtp", &values[..]))
    }

    /// Add source elements for an RTSP flow
    fn add_source_rtsp(&mut self) -> Result<(), Error> {
        let src = make_element("rtspsrc", None)?;
        set_property(&src, "location", &self.source.location)?;
        match &self.source.rtsp_transport {
            Transport::ANY => (),
            Transport::UDP => src.set_property_from_str("protocols", &"udp"),
            Transport::MCAST => {
                src.set_property_from_str("protocols", &"udp-mcast");
            },
            Transport::TCP => src.set_property_from_str("protocols", &"tcp"),
        }
        set_property(&src, "tcp-timeout", &self.source.timeout_us())?;
        // Retry TCP after UDP timeout (0 for disabled)
        set_property(&src, "timeout", &self.source.timeout_us())?;
        set_property(&src, "latency", &self.source.latency)?;
        set_property(&src, "do-retransmission", &false)?;
        set_property(&src, "user-agent", &AGENT)?;
        match src.connect("select-stream", false, |values| {
            match values[1].get::<u32>() {
                Ok(Some(num)) => Some((num == STREAM_NUM_VIDEO).to_value()),
                _ => None,
            }
        }) {
            Ok(_) => self.add_element(src),
            Err(_) => Err(Error::ConnectSignal("select-stream")),
        }
    }

    /// Add source elements for an HTTP flow
    fn add_source_http(&mut self) -> Result<(), Error> {
        let src = make_element("souphttpsrc", None)?;
        set_property(&src, "location", &self.location_http()?)?;
        // Blocking request timeout (0 for no timeout)
        set_property(&src, "timeout", &self.source.timeout_s())?;
        set_property(&src, "retries", &0)?;
        self.add_element(src)
    }

    /// Get HTTP location
    fn location_http(&self) -> Result<&str, Error> {
        match self.source.encoding {
            Encoding::PNG | Encoding::MJPEG => Ok(&self.source.location),
            _ => Err(Error::Other("invalid encoding for HTTP")),
        }
    }

    /// Add source element for a test flow
    fn add_source_test(&mut self) -> Result<(), Error> {
        let src = make_element("videotestsrc", None)?;
        src.set_property_from_str("pattern", "smpte75");
        set_property(&src, "is-live", &true)?;
        self.add_element(src)
    }

    /// Add decode elements
    fn add_decode(&mut self) -> Result<(), Error> {
        match self.source.encoding {
            Encoding::PNG => {
                self.add_element(make_element("imagefreeze", None)?)?;
                self.add_element(make_element("videoconvert", None)?)?;
                self.add_element(make_element("pngdec", None)?)
            }
            Encoding::MJPEG => self.add_element(make_element("jpegdec", None)?),
            Encoding::MPEG2 => {
                self.add_element(make_element("mpeg2dec", None)?)?;
                self.add_element(make_element("tsdemux", None)?)
            }
            Encoding::MPEG4 => self.add_element(self.create_mpeg4dec()?),
            Encoding::H264 => self.add_element(self.create_h264dec()?),
            Encoding::H265 => self.add_element(self.create_h265dec()?),
            Encoding::VP8 => self.add_element(self.create_vp8dec()?),
            Encoding::VP9 => self.add_element(self.create_vp9dec()?),
            Encoding::AV1 => self.add_element(make_element("av1dec", None)?),
            _ => Err(Error::Other("invalid encoding")),
        }
    }

    /// Add queue element
    fn add_queue(&mut self) -> Result<(), Error> {
        let que = make_element("queue", None)?;
        set_property(&que, "max-size-time", &SEC_NS)?;
        if self.needs_encode() {
            // leak (drop) packets -- when encoding cannot keep up
            que.set_property_from_str("leaky", &"downstream");
        }
        self.add_element(que)
    }

    /// Create MPEG-4 decode element
    fn create_mpeg4dec(&self) -> Result<Element, Error> {
        let dec = make_element("avdec_mpeg4", None)?;
        set_property(&dec, "output-corrupt", &false)?;
        Ok(dec)
    }

    /// Create h.264 decode element
    fn create_h264dec(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => make_element("vaapih264dec", None),
            Acceleration::OMX => make_element("omxh264dec", None),
            _ => {
                let dec = make_element("avdec_h264", None)?;
                set_property(&dec, "output-corrupt", &false)?;
                Ok(dec)
            }
        }
    }

    /// Create h.265 decode element
    fn create_h265dec(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => make_element("vaapih265dec", None),
            _ => make_element("libde265dec", None),
        }
    }

    /// Create VP8 decode element
    fn create_vp8dec(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => make_element("vaapivp8dec", None),
            Acceleration::OMX => make_element("omxvp8dec", None),
            _ => make_element("vp8dec", None),
        }
    }

    /// Create VP9 decode element
    fn create_vp9dec(&self) -> Result<Element, Error> {
        match self.acceleration {
            Acceleration::VAAPI => make_element("vaapivp9dec", None),
            _ => make_element("vp9dec", None),
        }
    }

    /// Create a sink element
    fn create_sink(&self) -> Result<Element, Error> {
        let sink = make_element(
            self.sink.factory_name(self.acceleration),
            Some("sink"),
        )?;
        match &self.sink {
            Sink::RTP(addr, port, _, _) => {
                set_property(&sink, "host", addr)?;
                set_property(&sink, "port", port)?;
                set_property(&sink, "ttl-mc", &TTL_MULTICAST)?;
            }
            Sink::WINDOW(crop) => {
                set_property(
                    &sink,
                    "force-aspect-ratio",
                    &crop.aspect.as_bool(),
                )?;
                if let Some(handle) = self.handle {
                    match sink.clone().dynamic_cast::<VideoOverlay>() {
                        Ok(overlay) => unsafe {
                            overlay.set_window_handle(handle);
                        },
                        Err(_) => error!("{}: invalid video overlay", self),
                    }
                }
            }
            _ => (),
        }
        Ok(sink)
    }

    /// Create a text overlay element
    fn create_text(&self) -> Result<Element, Error> {
        let txt = make_element("textoverlay", Some("txt"))?;
        set_property(&txt, "auto-resize", &false)?;
        set_property(&txt, "text", &self.overlay_text.as_ref().unwrap())?;
        set_property(&txt, "shaded-background", &false)?;
        set_property(&txt, "color", &OVERLAY_COLOR)?;
        txt.set_property_from_str("wrap-mode", &"none");
        txt.set_property_from_str("halignment", &"right");
        txt.set_property_from_str("valignment", &"top");
        Ok(txt)
    }

    /// Add an element to pipeline
    fn add_element(&mut self, elem: Element) -> Result<(), Error> {
        trace!("{}: add_element {}", self, elem.get_name());
        match self.pipeline.upgrade() {
            Some(pipeline) => {
                if let Err(_) = pipeline.add(&elem) {
                    return Err(Error::PipelineAdd());
                }
                match self.head.take() {
                    Some(head) => self.link_src_sink(&elem, head)?,
                    None => (),
                }
                self.head = Some(elem);
                Ok(())
            }
            None => Err(Error::Other("pipeline gone")),
        }
    }

    /// Link a source element with a sink
    fn link_src_sink(&self, src: &Element, sink: Element) -> Result<(), Error> {
        debug!("{}: {} => {}", self, src.get_name(), sink.get_name());
        match src.link(&sink) {
            Ok(()) => {
                let p0 = src.get_name();
                let p1 = sink.get_name();
                debug!("{}: pad linked (static) {} => {}", self, p0, p1);
            }
            Err(_) => {
                let sink = sink.downgrade(); // weak ref
                src.connect_pad_added(move |src, src_pad| {
                    match sink.upgrade() {
                        Some(sink) => link_ghost_pad(src, src_pad, sink),
                        None => error!("sink gone"),
                    }
                });
            }
        }
        Ok(())
    }

    /// Handle a bus message
    fn handle_message(&mut self, msg: &Message) -> glib::Continue {
        match msg.view() {
            MessageView::AsyncDone(_) => {
                self.is_playing = true;
                debug!("{}: playing", self);
                if let Some(fb) = &self.feedback {
                    if let Err(e) = fb.send(Feedback::Playing(self.idx)) {
                        error!("{}: send {}", self, e);
                    }
                }
            }
            MessageView::Eos(_) => {
                debug!("{}: end of stream", self);
                self.stop();
            }
            MessageView::StateChanged(chg) => {
                match (chg.get_current(), &chg.get_src()) {
                    (State::Playing, Some(src)) => {
                        if src.is::<Pipeline>() {
                            self.configure_playing();
                        }
                    }
                    (State::Null, _) => self.stopped(),
                    _ => (),
                }
            }
            MessageView::Error(err) => {
                debug!("{}: error {}", self, err.get_error());
                self.stop();
            }
            MessageView::Warning(wrn) => {
                debug!("{}: warning {}", self, wrn.get_error());
                self.stop();
            }
            MessageView::Element(elem) => {
                if let Some(obj) = elem.get_src() {
                    if obj.get_name() == "GstUDPSrcTimeout" {
                        debug!("{}: udpsrc timeout", self);
                        self.stop();
                    }
                }
            }
            MessageView::Application(_app) => self.update_packet_stats(),
            _ => (),
        };
        glib::Continue(true)
    }

    /// Stop the flow
    fn stop(&mut self) {
        if let Some(pipeline) = self.pipeline.upgrade() {
            pipeline.set_state(State::Null).unwrap();
        }
        self.stopped();
    }

    /// Provide feedback for stopped state
    fn stopped(&mut self) {
        if self.is_playing {
            self.is_playing = false;
            debug!("{}: stopped", self);
            if let Some(fb) = &self.feedback {
                if let Err(e) = fb.send(Feedback::Stopped(self.idx)) {
                    error!("{}: send {}", self, e);
                }
            }
        }
    }

    /// Configure elements when state is playing
    fn configure_playing(&self) {
        match self.pipeline.upgrade() {
            Some(pipeline) => {
                if self.has_text() {
                    self.configure_text(&pipeline);
                }
                let crop = self.sink.crop();
                if crop.is_cropped() {
                    self.configure_vbox(&pipeline, crop);
                }
            }
            None => error!("{}: pipeline gone", self),
        }
    }

    /// Configure text overlay element
    fn configure_text(&self, pipeline: &Pipeline) {
        if let Some(txt) = pipeline.get_by_name("txt") {
            match txt.get_static_pad("src") {
                Some(src_pad) => match src_pad.get_current_caps() {
                    Some(caps) => match self.config_txt_props(txt, caps) {
                        Err(_) => error!("{}: txt props", self),
                        _ => (),
                    },
                    None => error!("{}: no caps on txt src pad", self),
                },
                None => error!("{}: no txt src pad", self),
            }
        }
    }

    /// Configure text overlay properties
    fn config_txt_props(&self, txt: Element, caps: Caps) -> Result<(), Error> {
        for s in caps.iter() {
            if let Ok(Some(height)) = s.get::<i32>("height") {
                let sz = FONT_SZ * u32::try_from(height)? / DEFAULT_HEIGHT;
                let margin = i32::try_from(sz / 2)?;
                debug!("{}: font sz {}, height: {}", self, sz, height);
                let font = format!("Overpass, Bold {}", sz);
                set_property(&txt, "font-desc", &font)?;
                set_property(&txt, "ypad", &margin)?; // from top edge
                set_property(&txt, "xpad", &margin)?; // from right edge
            }
        }
        Ok(())
    }

    /// Configure videobox element
    fn configure_vbox(&self, pipeline: &Pipeline, crop: MatrixCrop) {
        if let Some(vbx) = pipeline.get_by_name("vbox") {
            match vbx.get_static_pad("src") {
                Some(src_pad) => match src_pad.get_current_caps() {
                    Some(caps) => {
                        match self.config_vbox_props(vbx, caps, crop) {
                            Err(_) => error!("{}: vbox props", self),
                            _ => (),
                        }
                    }
                    None => error!("{}: no caps on vbox src pad", self),
                },
                None => error!("{}: no vbox src pad", self),
            }
        }
    }

    /// Configure videobox properties
    fn config_vbox_props(
        &self, vbx: Element, caps: Caps, crop: MatrixCrop,
    ) -> Result<(), Error> {
        for s in caps.iter() {
            match (s.get("width"), s.get("height")) {
                (Ok(Some(width)), Ok(Some(height))) => {
                    set_property(&vbx, "top", &crop.top(height))?;
                    set_property(&vbx, "bottom", &crop.bottom(height))?;
                    set_property(&vbx, "left", &crop.left(width))?;
                    set_property(&vbx, "right", &crop.right(width))?;
                }
                _ => (),
            }
        }
        Ok(())
    }
    /// Update packet statistics
    fn update_packet_stats(&mut self) {
        let pushed = self.pushed;
        let lost = self.lost;
        let late = self.late;
        match self.pipeline.upgrade() {
            Some(pipeline) => {
                if let Some(jitter) = pipeline.get_by_name("jitter") {
                    if let Err(e) = self.update_jitter_stats(jitter) {
                        warn!("{}: jitter stats -- {}", self, e);
                    }
                }
            }
            None => error!("{}: pipeline gone", self),
        }
        if self.pushed >= pushed && self.lost >= lost && self.late >= late {
            if let Some(fb) = &self.feedback {
                let pushed = self.pushed - pushed;
                let lost = self.lost - lost;
                let late = self.late - late;
                if let Err(e) = fb.send(Feedback::Stats(self.idx, pushed, lost,
                    late))
                {
                    error!("{}: send {}", self, e);
                }
            }
        }
    }
    /// Get statistics from jitter buffer element
    fn update_jitter_stats(&mut self, jitter: Element) -> Result<(), Error> {
        let prop = jitter.get_property("stats")?;
        let stats = prop.get::<Structure>()?
            .ok_or(Error::Other("empty stats"))?;
        let pushed = stats.get::<u64>("num-pushed")?
            .ok_or(Error::Other("missing num-pushed"))?;
        let lost = stats.get::<u64>("num-lost")?
            .ok_or(Error::Other("missing num-lost"))?;
        let late = stats.get::<u64>("num-late")?
            .ok_or(Error::Other("missing num-late"))?;
        self.pushed = pushed;
        self.lost = lost;
        self.late = late;
        Ok(())
    }
}

impl Drop for Flow {
    fn drop(&mut self) {
        self.bus.remove_watch().unwrap();
        self.pipeline.set_state(State::Null).unwrap();
    }
}

impl fmt::Display for FlowChecker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Flow{}", self.idx)
    }
}

impl FlowChecker {
    /// Create a new periodic flow checker
    fn new(idx: usize, pipeline: WeakRef<Pipeline>) -> Self {
        FlowChecker {
            idx,
            pipeline,
            count: 0,
            last_pts: ClockTime::none(),
        }
    }
    /// Do periodic flow checks
    fn do_check(&mut self) -> glib::Continue {
        self.count += 1;
        match self.pipeline.upgrade() {
            Some(pipeline) => {
                if self.check_stopped(&pipeline) {
                    self.count = 0;
                    return glib::Continue(true);
                }
                if !self.post_stats(&pipeline) {
                    return glib::Continue(false);
                }
                self.check_sink(&pipeline)
            }
            None => {
                debug!("{}: do_check pipeline gone", self);
                glib::Continue(false)
            }
        }
    }
    /// Check if pipeline is stopped, and restart if necessary
    fn check_stopped(&self, pipeline: &Pipeline) -> bool {
        match pipeline.get_state(ClockTime::from_seconds(0)) {
            (_, State::Null, _) => {
                debug!("{}: restarting", self);
                pipeline.set_state(State::Playing).unwrap();
                true
            }
            _ => false,
        }
    }
    /// Post stats message
    fn post_stats(&self, pipeline: &Pipeline) -> bool {
        let structure = Structure::new_empty("stats");
        let msg = Message::new_application(structure).build();
        let bus = pipeline.get_bus().unwrap();
        match bus.post(&msg) {
            Ok(_) => true,
            Err(_) => {
                error!("{}: post_stats failed", self);
                false
            }
        }
    }
    /// Check sink in a pipeline
    fn check_sink(&mut self, pipeline: &Pipeline) -> glib::Continue {
        match pipeline.get_by_name("sink") {
            Some(sink) => {
                if self.is_stuck(&sink) {
                    let msg = Message::new_eos().src(Some(&sink)).build();
                    let bus = pipeline.get_bus().unwrap();
                    if let Err(_) = bus.post(&msg) {
                        error!("{}: check_sink post failed", self);
                        return glib::Continue(false);
                    }
                }
                glib::Continue(true)
            }
            None => {
                warn!("{}: check_sink sink gone", self);
                glib::Continue(false)
            }
        }
    }
    /// Check sink to make sure that last-sample is updating.
    fn is_stuck(&mut self, sink: &Element) -> bool {
        match sink.get_property("last-sample") {
            Ok(sample) => match sample.get::<Sample>() {
                Ok(Some(sample)) => match sample.get_buffer() {
                    Some(buffer) => {
                        let pts = buffer.get_pts();
                        trace!("{}: PTS {}", self, pts);
                        let stuck = pts == self.last_pts;
                        if stuck {
                            debug!("{}: PTS stuck @ {} -- posting EOS", self,
                                pts);
                        } else {
                            self.last_pts = pts;
                        }
                        return stuck;
                    }
                    None => error!("{}: sample buffer missing", self),
                },
                _ => debug!("{}: last-sample missing {}", self, self.count),
            },
            Err(_) => error!("{}: get last-sample failed", self),
        };
        self.count > PTS_CHECK_TRIES
    }
}

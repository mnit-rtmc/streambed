// stream.rs
//
// Copyright (C) 2019  Minnesota Department of Transportation
//
use crate::error::Error;
use glib::{Cast, ObjectExt, ToSendValue, ToValue, WeakRef};
use gstreamer::{
    Bus, Caps, ClockTime, Element, ElementExt, ElementExtManual, ElementFactory,
    GstBinExt, GstObjectExt, GObjectExtManualGst, Message, MessageView, PadExt,
    PadExtManual, Pipeline, Sample, State, Structure,
};
use gstreamer_video::{VideoOverlay, VideoOverlayExtManual};
use log::{debug, error, info, trace, warn};
use std::convert::TryFrom;
use std::fmt;

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

/// Font size (pt), using default height
const FONT_SZ: u32 = 12;

/// Default height (px)
const DEFAULT_HEIGHT: u32 = 240;

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
pub struct MatrixCrop {
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
    /// Video Acceleration API
    VAAPI(AspectRatio, Option<MatrixCrop>),
    /// X-Video Image
    XVIMAGE(AspectRatio, Option<MatrixCrop>),
}

/// Feedback on stream playing/stopped
pub trait Feedback: Send {
    /// Stream playing
    fn playing(&self);
    /// Stream stopped
    fn stopped(&self) -> bool;
}

/// Builder for video streams
#[derive(Default)]
pub struct StreamBuilder {
    /// Index of stream
    idx: usize,
    /// Video source config
    source: Source,
    /// Video sink config
    sink: Sink,
    /// Overlay text
    overlay_text: Option<String>,
    /// Stream feedback
    feedback: Option<Box<dyn Feedback>>,
    /// Video overlay handle
    handle: Option<usize>,
    /// Pipeline for stream
    pipeline: WeakRef<Pipeline>,
    /// Head element of pipeline
    head: Option<Element>,
}

/// Video stream
pub struct Stream {
    /// Video pipeline
    pipeline: Pipeline,
    /// Pipeline message bus
    bus: Bus,
    /// Number of pushed packets
    pushed: u64,
    /// Number of lost packets
    lost: u64,
    /// Number of late packets
    late: u64,
}

impl Default for AspectRatio {
    fn default() -> Self {
        AspectRatio::PRESERVE
    }
}

impl Default for Encoding {
    fn default() -> Self {
        Encoding::RAW
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

impl TryFrom<&str> for MatrixCrop {
    type Error = Error;

    fn try_from(v: &str) -> Result<Self, Self::Error> {
        let p: Vec<_> = v.split(',').collect();
        if p.len() == 3 {
            let mut crop = p[0].chars();
            let x = MatrixCrop::position(crop.next().unwrap_or_default())?;
            let y = MatrixCrop::position(crop.next().unwrap_or_default())?;
            let width = MatrixCrop::position(crop.next().unwrap_or_default())?
                + 1;
            let height = MatrixCrop::position(crop.next().unwrap_or_default())?
                + 1;
            if x < width && y < height {
                let hgap: u32 = p[1].parse()?;
                let vgap: u32 = p[2].parse()?;
                // Don't allow more than 50% gap
                if hgap <= MatrixCrop::PERCENT / 2 &&
                   vgap <= MatrixCrop::PERCENT / 2
                {
                    return Ok(MatrixCrop { x, y, width, height, hgap, vgap });
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
            'A' ..= 'H' => Ok(c as u8 - b'A'),
            _ => Err(Error::InvalidCrop()),
        }
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
    fn factory_name(&self) -> &'static str {
        match self {
            Sink::FAKE => "fakesink",
            Sink::RTP(_, _, _, _) => "udpsink",
            Sink::VAAPI(_, _) => "vaapisink",
            Sink::XVIMAGE(_, _) => "xvimagesink",
        }
    }
    /// Get the aspect ratio setting
    fn aspect_ratio(&self) -> Option<AspectRatio> {
        match self {
            Sink::VAAPI(a, _) => Some(*a),
            Sink::XVIMAGE(a, _) => Some(*a),
            _ => None,
        }
    }
    /// Get the matrix crop setting
    fn crop(&self) -> &Option<MatrixCrop> {
        match self {
            Sink::VAAPI(_, c) => &c,
            Sink::XVIMAGE(_, c) => &c,
            _ => &None,
        }
    }
    /// Get the sink encoding
    fn encoding(&self) -> Encoding {
        match self {
            Sink::RTP(_, _, encoding, _) => *encoding,
            _ => Encoding::RAW,
        }
    }
}

impl fmt::Display for StreamBuilder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Stream{} {}", self.idx, self.source.location)
    }
}

impl StreamBuilder {

    /// Create a new stream builder
    pub fn new(idx: usize) -> Self {
        StreamBuilder {
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

    /// Use the specified overlay text
    pub fn with_overlay_text(mut self, overlay_text: Option<&str>) -> Self {
        self.overlay_text = overlay_text.map(|t| t.to_string());
        self
    }

    /// Use the specified stream feedback
    pub fn with_feedback(mut self, feedback: Option<Box<dyn Feedback>>)
        -> Self
    {
        self.feedback = feedback;
        self
    }

    /// Use the specified video overlay window handle
    pub fn with_handle(mut self, handle: Option<usize>) -> Self {
        self.handle = handle;
        self
    }

    /// Build the stream
    pub fn build(mut self) -> Result<Stream, Error> {
        let name = format!("m{}", self.idx);
        let pipeline = Pipeline::new(Some(&name));
        self.pipeline = pipeline.downgrade();
        self.add_elements()?;
        let timeout_ms = self.source.timeout_ms();
        let bus = pipeline.get_bus().unwrap();
        bus.add_watch(move |_bus, m| self.handle_message(m));
        let mut check = StreamCheck::new(pipeline.downgrade());
        glib::source::timeout_add(timeout_ms, move || check.pts_check());
        pipeline.set_state(State::Playing).unwrap();
        Ok(Stream {
            pipeline,
            bus,
            pushed: 0,
            lost: 0,
            late: 0,
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
        if self.sink.crop().is_some() {
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
        self.source.is_rtp() && !self.is_rtp_passthru()
    }

    /// Check if RTP can pass unchanged from source to sink
    fn is_rtp_passthru(&self) -> bool {
        self.source.is_rtp() && self.sink.is_rtp() && !self.needs_transcode()
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
        let pay = make_element(self.source.encoding.rtp_pay()?, None)?;
        if let Sink::RTP(_, _, _, true) = self.sink {
            match self.sink.encoding() {
                Encoding::MPEG4 => {
                    // send configuration headers once per second
                    pay.set_property("config-interval", &1u32)?;
                }
                Encoding::H264 | Encoding::H265 => {
                    // send sprop parameter sets every IDR frame (-1)
                    pay.set_property("config-interval", &(-1))?;
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
            Encoding::MPEG4 => {
                self.add_element(make_element("avenc_mpeg4", None)?)
            }
            Encoding::H264 => {
                let enc = make_element("x264enc", None)?;
                enc.set_property_from_str("tune", &"zerolatency");
                // With the default "medium" speed-preset, the pipeline can't
                // run live.  With "superfast", the quality is still very good.
                // ultrafast (1), superfast (2), veryfast (3), faster (4),
                // fast (5), medium (6), etc.
                enc.set_property_from_str("speed-preset", &"superfast");
                self.add_element(enc)
            },
            Encoding::H265 => {
                let enc = make_element("x265enc", None)?;
                enc.set_property_from_str("tune", &"zerolatency");
                enc.set_property_from_str("speed-preset", &"superfast");
                self.add_element(enc)
            },
            Encoding::VP8 => self.add_element(make_element("vp8enc", None)?),
            Encoding::VP9 => self.add_element(make_element("vp9enc", None)?),
            Encoding::AV1 => self.add_element(make_element("av1enc", None)?),
            _ => Err(Error::Other("invalid encoding")),
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
            Err(Error::Other("invalid location"))
        }
    }

    /// Add source elements for an RTP stream
    fn add_source_rtp(&mut self) -> Result<(), Error> {
        if !self.source.is_rtsp() {
            let jtr = make_element("rtpjitterbuffer", Some("jitter"))?;
            jtr.set_property("latency", &self.source.latency)?;
            jtr.set_property("max-dropout-time", &self.source.timeout_ms())?;
            self.add_element(jtr)?;
            let fltr = make_element("capsfilter", None)?;
            let caps = self.create_rtp_caps()?;
            fltr.set_property("caps", &caps)?;
            self.add_element(fltr)?;
        }
        let src = make_element("udpsrc", None)?;
        src.set_property("uri", &self.source.location)?;
        // Post GstUDPSrcTimeout messages after timeout (0 for disabled)
        src.set_property("timeout", &self.source.timeout_ns())?;
        self.add_element(src)
    }

    /// Create RTP caps for filter element
    fn create_rtp_caps(&self) -> Result<Caps, Error> {
        let mut values: Vec<(&str, &dyn ToSendValue)> =
            vec![("clock-rate", &90_000)];
        if let Encoding::MPEG2 = self.source.encoding {
            values.push(("encoding-name", &"MP2T"));
        }
        if let Some(sprops) = &self.source.sprops {
            values.push(("sprop-parameter-sets", &sprops));
            return Ok(Caps::new_simple("application/x-rtp", &values[..]));
        }
        Ok(Caps::new_simple("application/x-rtp", &values[..]))
    }

    /// Add source elements for an RTSP stream
    fn add_source_rtsp(&mut self) -> Result<(), Error> {
        let src = make_element("rtspsrc", None)?;
        src.set_property("location", &self.source.location)?;
        src.set_property("tcp-timeout", &(2 * self.source.timeout_us()))?;
        // Retry TCP after UDP timeout (0 for disabled)
        src.set_property("timeout", &self.source.timeout_us())?;
        src.set_property("latency", &self.source.latency)?;
        src.set_property("do-retransmission", &false)?;
        src.connect("select-stream", false, |values| {
            let num = values[1].get::<u32>().unwrap();
            Some((num == STREAM_NUM_VIDEO).to_value())
        })?;
        self.add_element(src)
    }

    /// Add source elements for an HTTP stream
    fn add_source_http(&mut self) -> Result<(), Error> {
        let src = make_element("souphttpsrc", None)?;
        src.set_property("location", &self.location_http()?)?;
        // Blocking request timeout (0 for no timeout)
        src.set_property("timeout", &self.source.timeout_s())?;
        src.set_property("retries", &0)?;
        self.add_element(src)
    }

    /// Get HTTP location
    fn location_http(&self) -> Result<&str, Error> {
        match self.source.encoding {
            Encoding::PNG | Encoding::MJPEG => Ok(&self.source.location),
            _ => Err(Error::Other("invalid encoding for HTTP")),
        }
    }

    /// Add decode elements
    fn add_decode(&mut self) -> Result<(), Error> {
        match self.source.encoding {
            Encoding::PNG => {
                self.add_element(make_element("imagefreeze", None)?)?;
                self.add_element(make_element("videoconvert", None)?)?;
                self.add_element(make_element("pngdec", None)?)
            }
            Encoding::MJPEG => {
                self.add_element(make_element("jpegdec", None)?)
            }
            Encoding::MPEG2 => {
                self.add_element(make_element("mpeg2dec", None)?)?;
                self.add_element(make_element("tsdemux", None)?)
            }
            Encoding::MPEG4 => {
                let dec = make_element("avdec_mpeg4", None)?;
                dec.set_property("output-corrupt", &false)?;
                self.add_element(dec)
            }
            Encoding::H264 => self.add_element(self.create_h264dec()?),
            Encoding::H265 => {
                self.add_element(make_element("libde265dec", None)?)
            }
            Encoding::VP8 => self.add_element(make_element("vp8dec", None)?),
            Encoding::VP9 => self.add_element(make_element("vp9dec", None)?),
            Encoding::AV1 => self.add_element(make_element("av1dec", None)?),
            _ => Err(Error::Other("invalid encoding")),
        }
    }

    /// Add queue element
    fn add_queue(&mut self) -> Result<(), Error> {
        let que = make_element("queue", None)?;
        que.set_property("max-size-time", &SEC_NS)?;
        if self.needs_encode() {
            // leak (drop) packets -- when encoding cannot keep up
            que.set_property_from_str("leaky", &"downstream");
        }
        self.add_element(que)
    }

    /// Create H264 decode element
    fn create_h264dec(&self) -> Result<Element, Error> {
        match self.sink {
            Sink::VAAPI(_, _) => make_element("vaapih264dec", None),
            _ => {
                let dec = make_element("avdec_h264", None)?;
                dec.set_property("output-corrupt", &false)?;
                Ok(dec)
            }
        }
    }

    /// Create a sink element
    fn create_sink(&self) -> Result<Element, Error> {
        let sink = make_element(self.sink.factory_name(), Some("sink"))?;
        if let Some(aspect) = self.sink.aspect_ratio() {
            sink.set_property("force-aspect-ratio", &aspect.as_bool())?;
        }
        match &self.sink {
            Sink::RTP(addr, port, _, _) => {
                sink.set_property("host", addr)?;
                sink.set_property("port", port)?;
                sink.set_property("ttl-mc", &15)?;
            }
            Sink::VAAPI(_, _) | Sink::XVIMAGE(_, _) => {
                if let Some(handle) = self.handle {
                    match sink.clone().dynamic_cast::<VideoOverlay>() {
                        Ok(overlay) => unsafe {
                            overlay.set_window_handle(handle);
                        }
                        Err(_) => error!("invalid video overlay"),
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
        txt.set_property("auto-resize", &false)?;
        txt.set_property("text", &self.overlay_text.as_ref().unwrap())?;
        txt.set_property("shaded-background", &false)?;
        txt.set_property("color", &0xFF_FF_FF_E0u32)?; // yellowish white
        txt.set_property_from_str("wrap-mode", &"none");
        txt.set_property_from_str("halignment", &"right");
        txt.set_property_from_str("valignment", &"top");
        Ok(txt)
    }

    /// Add an element to pipeline
    fn add_element(&mut self, elem: Element) -> Result<(), Error> {
        trace!("add_element: {} -- {}", elem.get_name(), self);
        match self.pipeline.upgrade() {
            Some(pipeline) => {
                pipeline.add(&elem)?;
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
        debug!("link_src_sink: {} => {} -- {}", src.get_name(), sink.get_name(),
            self);
        src.link(&sink)?;
        let sink = sink.downgrade(); // weak ref
        src.connect_pad_added(move |src, src_pad| {
            match sink.upgrade() {
                Some(sink) => {
                    match sink.get_static_pad("sink") {
                        Some(sink_pad) => {
                            let p0 = src.get_name();
                            let p1 = sink.get_name();
                            match src_pad.link(&sink_pad) {
                                Ok(_) => debug!("pad linked: {} => {}", p0, p1),
                                Err(_) => error!("pad link: {}", p0),
                            }
                        }
                        None => error!("no sink pad"),
                    }
                }
                None => error!("no sink to link"),
            }
        });
        Ok(())
    }

    /// Handle a bus message
    fn handle_message(&self, msg: &Message) -> glib::Continue {
        match msg.view() {
            MessageView::AsyncDone(_) => {
                info!("playing -- {}", self);
                if let Some(feedback) = &self.feedback {
                    feedback.playing();
                }
            }
            MessageView::Eos(_) => {
                warn!("end of stream -- {}", self);
                self.stop();
            }
            MessageView::StateChanged(chg) => {
                match (chg.get_current(), &chg.get_src()) {
                    (State::Playing, Some(src)) => {
                        if src.is::<Pipeline>() {
                            self.configure_playing();
                        }
                    }
                    _ => (),
                }
            }
            MessageView::Error(err) => {
                error!("{} -- {}", err.get_error(), self);
                self.stop();
            }
            MessageView::Warning(wrn) => {
                warn!("{} -- {}", wrn.get_error(), self);
                self.stop();
            }
            MessageView::Element(elem) => {
                if let Some(obj) = elem.get_src() {
                    if obj.get_name() == "GstUDPSrcTimeout" {
                        error!("udpsrc timeout -- {}", self);
                        self.stop();
                    }
                }
            }
            _ => (),
        };
        glib::Continue(true)
    }

    /// Play the stream
    fn play(&self) {
        if let Some(pipeline) = self.pipeline.upgrade() {
            pipeline.set_state(State::Playing).unwrap();
        }
    }

    /// Stop the stream
    fn stop(&self) {
        if let Some(pipeline) = self.pipeline.upgrade() {
            pipeline.set_state(State::Null).unwrap();
            info!("stopped -- {}", self);
        }
        if let Some(feedback) = &self.feedback {
            if feedback.stopped() {
                self.play();
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
                if let Some(crop) = self.sink.crop() {
                    self.configure_vbox(&pipeline, &crop);
                }
            }
            None => error!("pipeline gone -- {}", self),
        }
    }

    /// Configure text overlay element
    fn configure_text(&self, pipeline: &Pipeline) {
        if let Some(txt) = pipeline.get_by_name("txt") {
            match txt.get_static_pad("src") {
                Some(src_pad) => {
                    match src_pad.get_current_caps() {
                        Some(caps) => {
                            match self.config_txt_props(txt, caps) {
                                Err(_) => error!("txt props -- {}", self),
                                _ => (),
                            }
                        }
                        None => error!("no caps on txt src pad -- {}", self),
                    }
                }
                None => error!("no txt src pad -- {}", self),
            }
        }
    }

    /// Configure text overlay properties
    fn config_txt_props(&self, txt: Element, caps: Caps) -> Result<(), Error> {
        for s in caps.iter() {
            match s.get::<i32>("height") {
                Some(height) => {
                    let sz = FONT_SZ * u32::try_from(height)? / DEFAULT_HEIGHT;
                    let margin = i32::try_from(sz / 2)?;
                    debug!("font sz: {}, height: {} -- {}", sz, height, self);
                    let font = format!("Overpass, Bold {}", sz);
                    txt.set_property("font-desc", &font)?;
                    txt.set_property("ypad", &margin)?; // from top edge
                    txt.set_property("xpad", &margin)?; // from right edge
                }
                _ => (),
            }
        }
        Ok(())
    }

    /// Configure videobox element
    fn configure_vbox(&self, pipeline: &Pipeline, crop: &MatrixCrop) {
        if let Some(vbx) = pipeline.get_by_name("vbox") {
            match vbx.get_static_pad("src") {
                Some(src_pad) => {
                    match src_pad.get_current_caps() {
                        Some(caps) => {
                            match self.config_vbox_props(vbx, caps, crop) {
                                Err(_) => error!("vbox props -- {}", self),
                                _ => (),
                            }
                        }
                        None => error!("no caps on vbox src pad -- {}", self),
                    }
                }
                None => error!("no vbox src pad -- {}", self),
            }
        }
    }

    /// Configure videobox properties
    fn config_vbox_props(&self, vbx: Element, caps: Caps, crop: &MatrixCrop)
        -> Result<(), Error>
    {
        for s in caps.iter() {
            match (s.get("width"), s.get("height")) {
                (Some(width), Some(height)) => {
                    vbx.set_property("top", &crop.top(height))?;
                    vbx.set_property("bottom", &crop.bottom(height))?;
                    vbx.set_property("left", &crop.left(width))?;
                    vbx.set_property("right", &crop.right(width))?;
                }
                _ => (),
            }
        }
        Ok(())
    }
}

/// Make a pipeline element
fn make_element(factory_name: &'static str, name: Option<&str>)
    -> Result<Element, Error>
{
    match ElementFactory::make(factory_name, name) {
        Some(elem) => Ok(elem),
        None => {
            error!("make_element: {}", factory_name);
            Err(Error::Other(factory_name))
        }
    }
}

/// Stream PTS stuck check
struct StreamCheck {
    /// Stream pipeline
    pipeline: WeakRef<Pipeline>,
    /// Most recent presentation time stamp
    last_pts: ClockTime,
}

impl StreamCheck {
    /// Create a new stream PTS stuck check
    fn new(pipeline: WeakRef<Pipeline>) -> Self {
        StreamCheck {
            pipeline,
            last_pts: ClockTime::none(),
        }
    }
    /// Check if PTS has stopped updating
    ///
    /// Post an EOS message if necessary.
    fn pts_check(&mut self) -> glib::Continue {
        if let Some(pipeline) = self.pipeline.upgrade() {
            if let Some(sink) = pipeline.get_by_name("sink") {
                match self.check_sink(&sink) {
                    Ok(true) => {
                        let msg = Message::new_eos().src(Some(&sink)).build();
                        let bus = pipeline.get_bus().unwrap();
                        match bus.post(&msg) {
                            Ok(_) => return glib::Continue(true),
                            Err(_) => error!("pts_check bus.post failed"),
                        }
                    }
                    Ok(false) => return glib::Continue(true),
                    Err(_) => return glib::Continue(false),
                }
            }
        }
        warn!("pts_check failed");
        glib::Continue(false)
    }
    /// Check sink to make sure that last-sample is updating.
    fn check_sink(&mut self, sink: &Element) -> Result<bool, Error> {
        match sink.get_property("last-sample") {
            Ok(sample) => {
                match sample.get::<Sample>() {
                    Some(sample) => {
                        match sample.get_buffer() {
                            Some(buffer) => {
                                let pts = buffer.get_pts();
                                debug!("PTS: {}", pts);
                                let stuck = pts == self.last_pts;
                                if stuck {
                                    warn!("PTS stuck at {}; posting EOS", pts);
                                } else {
                                    self.last_pts = pts;
                                }
                                return Ok(stuck);
                            }
                            None => error!("sample buffer missing"),
                        }
                    }
                    None => error!("last-sample missing"),
                }
            }
            Err(_) => error!("get last-sample failed"),
        };
        Err(Error::Other("check_sink failed"))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
        self.bus.remove_watch().unwrap();
    }
}

impl Stream {

    /// Get packet statistics
    pub fn packet_stats(&mut self) -> Option<(u64, u64, u64)> {
        let pushed = self.pushed;
        let lost = self.lost;
        let late = self.late;
        let update = self.update_stats();
        if update &&
           self.pushed >= pushed &&
           self.lost >= lost &&
           self.late >= late
        {
            Some((
                 self.pushed - pushed,
                 self.lost - lost,
                 self.late - late,
            ))
        } else {
            None
        }
    }

    /// Update packet statistics
    fn update_stats(&mut self) -> bool {
        match self.pipeline.get_by_name("jitter") {
            Some(jitter) => self.jitter_stats(jitter),
            None => false,
        }
    }

    /// Get statistics from jitter buffer element
    fn jitter_stats(&mut self, jitter: Element) -> bool {
        match jitter.get_property("stats") {
            Ok(stats) => {
                match stats.get::<Structure>() {
                    Some(stats) => {
                        let pushed = stats.get::<u64>("num-pushed");
                        let lost = stats.get::<u64>("num-lost");
                        let late = stats.get::<u64>("num-late");
                        match (pushed, lost, late) {
                            (Some(pushed), Some(lost), Some(late)) => {
                                self.pushed = pushed;
                                self.lost = lost;
                                self.late = late;
                            }
                            _ => warn!("stats empty"),
                        }
                    }
                    None => warn!("missing stats"),
                }
            }
            Err(_) => warn!("failed to get jitter stats"),
        }
        false
    }

    /// Stop the stream
    pub fn stop(&self) {
        self.pipeline.set_state(State::Null).unwrap();
    }
}

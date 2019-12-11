// stream.rs
//
// Copyright (C) 2019  Minnesota Department of Transportation
//
use crate::error::Error;
use glib::{Cast, ObjectExt, ToSendValue, ToValue, WeakRef};
use gstreamer::{
    Bus, Caps, ClockTime, Element, ElementExt, ElementExtManual, ElementFactory,
    GstBinExt, GstObjectExt, Message, MessageView, PadExt, PadExtManual,
    Pipeline, Sample, State, Structure,
};
use gstreamer_video::{VideoOverlay, VideoOverlayExtManual};
use log::{error, info, warn};
use std::convert::TryFrom;

/// One second (microsecond units)
const SEC_US: u64 = 1_000_000;

/// One second (nanosecond units)
const SEC_NS: u64 = 1_000_000_000;

/// RTSP stream number for video
const STREAM_NUM_VIDEO: u32 = 0;

/// Default stream latency (ms)
const DEFAULT_LATENCY_MS: u32 = 100;

/// Default source timeout (sec)
const DEFAULT_TIMEOUT_SEC: u16 = 2;

/// Default font size (pt)
const DEFAULT_FONT_SZ: u32 = 22;

/// Pixel aspect ratio handling
pub enum AspectRatio {
    /// Adjust pixel aspect ratio to fill sink window
    FILL,
    /// Preserve pixel aspect ratio
    PRESERVE,
}

/// Video sink
pub enum Sink {
    /// Fake sink (for testing)
    FAKE,
    /// UDP multicasting (mcast_addr, mcast_port, insert_config)
    UDP(String, i32, bool),
    /// Video Acceleration API
    VAAPI,
    /// X-Video Image
    XVIMAGE,
}

/// Video encoding
#[derive(Debug)]
pub enum Encoding {
    /// Portable Network Graphics
    PNG,
    /// Motion JPEG
    MJPEG,
    /// MPEG-2 TS
    MPEG2,
    /// MPEG-4 with RTP
    MPEG4,
    /// H.264 with RTP
    H264,
    /// H.265 with RTP (future)
    H265,
    /// AV1 with RTP (future)
    AV1,
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

/// Builder for video streams
#[derive(Default)]
pub struct StreamBuilder {
    /// Index of stream
    idx: usize,
    /// Source location URL
    location: String,
    /// Video encoding
    encoding: Encoding,
    /// Stream properties (from SDP)
    sprops: Option<String>,
    /// Source timeout (sec)
    timeout: u16,
    /// Sink config
    sink: Sink,
    /// Latency (ms)
    latency: u32,
    /// Overlay text
    overlay_text: Option<String>,
    /// Font size (pt)
    font_sz: u32,
    /// Aspect ratio
    aspect: AspectRatio,
    /// Matrix crop configuration
    crop: Option<MatrixCrop>,
    /// Stream control
    control: Option<Box<dyn StreamControl>>,
    /// Pipeline for stream
    pipeline: Option<Pipeline>,
    /// Head element of pipeline
    head: Option<Element>,
}

/// Stream control receives feedback on start/stop
pub trait StreamControl: Send {
    /// Stream started
    fn started(&self);
    /// Stream stopped
    fn stopped(&self);
}

/// Video stream
pub struct Stream {
    /// Video pipeline
    pipeline: Pipeline,
    /// Pipeline message bus
    bus: Bus,
    /// Most recent presentation time stamp
    last_pts: ClockTime,
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

impl Default for Sink {
    fn default() -> Self {
        Sink::FAKE
    }
}

impl Sink {
    /// Get the gstreamer factory name
    fn factory_name(&self) -> &'static str {
        match self {
            Sink::FAKE => "fakesink",
            Sink::UDP(_, _, _) => "udpsink",
            Sink::VAAPI => "vaapisink",
            Sink::XVIMAGE => "xvimagesink",
        }
    }
    /// Does the sink have a window context
    fn has_window(&self) -> bool {
        match self {
            Sink::FAKE | Sink::UDP(_, _, _) => false,
            Sink::VAAPI | Sink::XVIMAGE => true,
        }
    }
}

impl Default for Encoding {
    fn default() -> Self {
        Encoding::PNG
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
        pix + gap
    }

    /// Get number of pixels to crop from bottom edge
    fn bottom(&self, height: u32) -> u32 {
        let num = u32::from(self.height - self.y - 1);
        let den = u32::from(self.height);
        let pix = height * num / den;
        let gap = height * self.vgap / (den * MatrixCrop::PERCENT * 2);
        pix + gap
    }

    /// Get number of pixels to crop from left edge
    fn left(&self, width: u32) -> u32 {
        let num = u32::from(self.x);
        let den = u32::from(self.width);
        let pix = width * num / den;
        let gap = width * self.hgap / (den * MatrixCrop::PERCENT * 2);
        pix + gap
    }

    /// Get number of pixels to crop from right edge
    fn right(&self, width: u32) -> u32 {
        let num = u32::from(self.width - self.x - 1);
        let den = u32::from(self.width);
        let pix = width * num / den;
        let gap = width * self.hgap / (den * MatrixCrop::PERCENT * 2);
        pix + gap
    }
}

impl StreamBuilder {

    /// Create a new stream builder
    pub fn new(idx: usize) -> Self {
        StreamBuilder {
            idx,
            timeout: DEFAULT_TIMEOUT_SEC,
            latency: DEFAULT_LATENCY_MS,
            font_sz: DEFAULT_FONT_SZ,
            ..Default::default()
        }
    }

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

    /// Use the specified sink type
    pub fn with_sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Use the specified latency (ms)
    pub fn with_latency(mut self, latency: u32) -> Self {
        self.latency = latency;
        self
    }

    /// Use the specified overlay text
    pub fn with_overlay_text(mut self, overlay_text: Option<&str>) -> Self {
        self.overlay_text = overlay_text.map(|t| t.to_string());
        self
    }

    /// Use the specified font size
    pub fn with_font_size(mut self, sz: u32) -> Self {
        self.font_sz = sz;
        self
    }

    /// Use the specified pixel aspect ratio
    pub fn with_aspect(mut self, aspect: AspectRatio) -> Self {
        self.aspect = aspect;
        self
    }

    /// Use the specified crop code
    pub fn with_crop(mut self, crop: Option<MatrixCrop>) -> Self {
        self.crop = crop;
        self
    }

    /// Use the specified stream control
    pub fn with_control(mut self, control: Option<Box<dyn StreamControl>>)
        -> Self
    {
        self.control = control;
        self
    }

    /// Build the stream
    pub fn build(mut self) -> Result<Stream, Error> {
        let name = format!("m{}", self.idx);
        self.pipeline = Some(Pipeline::new(Some(&name)));
        self.add_elements()?;
        let pipeline = self.pipeline.take().unwrap();
        let bus = pipeline.get_bus().unwrap();
        let pipeline_weak = pipeline.downgrade();
        let stream = Stream {
            pipeline,
            bus: bus.clone(),
            last_pts: ClockTime::none(),
            pushed: 0,
            lost: 0,
            late: 0,
        };
        bus.add_watch(move |b, m| self.bus_message(b, m, &pipeline_weak));
        Ok(stream)
    }

    /// Check if pipeline should have a text overlay
    fn has_text(&self) -> bool {
        match self.encoding {
            // NOTE: MJPEG and textoverlay don't play well together
            //       due to timestamp issues.
            Encoding::MJPEG => false,
            _ => self.overlay_text.is_some(),
        }
    }

    /// Add all required elements to the pipeline
    ///
    /// Pipeline is built from sink to source.
    fn add_elements(&mut self) -> Result<(), Error> {
        self.add_element(self.create_sink()?)?;
        if self.has_text() {
            self.add_element(self.create_text()?)?;
        }
        if self.crop.is_some() {
            self.add_element(make_element("videobox", Some("vbox"))?)?;
        }
        self.add_decode()?;
        self.add_source()?;
        Ok(())
    }

    /// Add source elements
    fn add_source(&mut self) -> Result<(), Error> {
        if self.location.starts_with("udp://") {
            let jtr = make_element("rtpjitterbuffer", Some("jitter"))?;
            jtr.set_property("latency", &self.latency)?;
            jtr.set_property("max-dropout-time", &1500)?;
            self.add_element(jtr)?;
            let fltr = make_element("capsfilter", None)?;
            let caps = self.create_rtp_caps()?;
            fltr.set_property("caps", &caps)?;
            self.add_element(fltr)?;
            let src = make_element("udpsrc", None)?;
            src.set_property("uri", &self.location)?;
            // Post GstUDPSrcTimeout messages after timeout (0 for disabled)
            src.set_property("timeout", &(u64::from(self.timeout) * SEC_NS))?;
            self.add_element(src)
        } else if self.location.starts_with("http://") {
            let src = make_element("souphttpsrc", None)?;
            src.set_property("location", &self.location_http()?)?;
            // Blocking request timeout (0 for no timeout)
            src.set_property("timeout", &u32::from(self.timeout))?;
            src.set_property("retries", &0)?;
            self.add_element(src)
        } else if self.location.starts_with("rtsp://") {
            let src = make_element("rtspsrc", None)?;
            src.set_property("location", &self.location)?;
            src.set_property("latency", &self.latency)?;
            src.set_property("tcp-timeout", &(10 * SEC_US))?;
            // Retry TCP after UDP timeout (0 for disabled)
            src.set_property("timeout", &(u64::from(self.timeout) * SEC_US))?;
            src.set_property("do-retransmission", &false)?;
            src.connect("select-stream", false, |values| {
                let num = values[1].get::<u32>().unwrap();
                Some((num == STREAM_NUM_VIDEO).to_value())
            })?;
            self.add_element(src)
        } else {
            Err(Error::Other("invalid location"))
        }
    }

    /// Create RTP caps for filter element
    fn create_rtp_caps(&self) -> Result<Caps, Error> {
        let mut values: Vec<(&str, &dyn ToSendValue)> =
            vec![("clock-rate", &90_000)];
        if let Encoding::MPEG2 = self.encoding {
            values.push(("encoding-name", &"MP2T"));
        }
        if let Some(sprops) = &self.sprops {
            values.push(("sprop-parameter-sets", &sprops));
            return Ok(Caps::new_simple("application/x-rtp", &values[..]));
        }
        Ok(Caps::new_simple("application/x-rtp", &values[..]))
    }

    /// Get HTTP location
    fn location_http(&self) -> Result<&str, Error> {
        match self.encoding {
            Encoding::PNG | Encoding::MJPEG => Ok(&self.location),
            _ => Err(Error::Other("invalid encoding for HTTP")),
        }
    }

    /// Add decoder / depayloader elements
    fn add_decode(&mut self) -> Result<(), Error> {
        match self.encoding {
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
                self.add_element(make_element("tsdemux", None)?)?;
                self.add_element(make_element("rtpmp2tdepay", None)?)?;
                let que = make_element("queue", None)?;
                que.set_property("max-size-time", &650_000_000)?;
                self.add_element(que)
            }
            Encoding::MPEG4 => {
                match self.sink {
                    Sink::UDP(_, _, true) => {
                        let pay = make_element("rtpmp4vpay", None)?;
                        // send configuration headers once per second
                        pay.set_property("config-interval", &1u32)?;
                        self.add_element(pay)?;
                        self.add_element(make_element("rtpmp4vdepay", None)?)
                    }
                    // don't need to depay for UDP
                    Sink::UDP(_, _, false) => Ok(()),
                    _ => {
                        let dec = make_element("avdec_mpeg4", None)?;
                        dec.set_property("output-corrupt", &false)?;
                        self.add_element(dec)?;
                        self.add_element(make_element("rtpmp4vdepay", None)?)
                    }
                }
            }
            Encoding::H264 => {
                match self.sink {
                    Sink::UDP(_, _, true) => {
                        let pay = make_element("rtph264pay", None)?;
                        // send sprop parameter sets every IDR frame (-1)
                        pay.set_property("config-interval", &(-1))?;
                        self.add_element(pay)?;
                        self.add_element(make_element("rtph264depay", None)?)
                    }
                    // don't need to depay for UDP
                    Sink::UDP(_, _, false) => Ok(()),
                    _ => {
                        self.add_element(self.create_h264dec()?)?;
                        self.add_element(make_element("rtph264depay", None)?)
                    }
                }
            },
            _ => Err(Error::Other("invalid encoding")),
        }
    }

    /// Create H264 decode element
    fn create_h264dec(&self) -> Result<Element, Error> {
        match self.sink {
            Sink::VAAPI => make_element("vaapih264dec", None),
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
        if self.sink.has_window() {
            sink.set_property("force-aspect-ratio", &self.aspect.as_bool())?;
        }
        match &self.sink {
            Sink::UDP(addr, port, _) => {
                sink.set_property("host", addr)?;
                sink.set_property("port", port)?;
                sink.set_property("ttl-mc", &15)?;
            }
            _ => (),
        }
        Ok(sink)
    }

    /// Create a text overlay element
    fn create_text(&self) -> Result<Element, Error> {
        let font = format!("Overpass, Bold {}", self.font_sz);
        let txt = make_element("textoverlay", None)?;
        txt.set_property("text", &self.overlay_text.as_ref().unwrap())?;
        txt.set_property("font-desc", &font)?;
        txt.set_property("shaded-background", &false)?;
        txt.set_property("color", &0xFF_FF_FF_E0u32)?;
        txt.set_property("halignment", &0i32)?; // left
        txt.set_property("valignment", &2i32)?; // top
        txt.set_property("wrap-mode", &(-1i32))?; // no wrapping
        txt.set_property("xpad", &48i32)?;
        txt.set_property("ypad", &36i32)?;
        Ok(txt)
    }

    /// Add an element to pipeline
    fn add_element(&mut self, elem: Element) -> Result<(), Error> {
        let pipeline = self.pipeline.as_ref().unwrap();
        pipeline.add(&elem)?;
        match self.head.take() {
            Some(head) => {
                link_src_sink(&elem, head)?;
                self.head = Some(elem);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Handle bus messages
    fn bus_message(&self, _bus: &Bus, msg: &Message, 
        pipeline: &WeakRef<Pipeline>) -> glib::Continue
    {
        match msg.view() {
            MessageView::AsyncDone(_) => {
                if let Some(control) = &self.control {
                    control.started();
                }
            }
            MessageView::Eos(_) => {
                warn!("End of stream: {}", self.location);
                self.stop();
            }
            MessageView::StateChanged(change) => {
                match (&self.crop, change.get_current(), &change.get_src()) {
                    (Some(crop), State::Playing, Some(src)) => {
                        if src.is::<Pipeline>() {
                            match pipeline.upgrade() {
                                Some(pipeline) => {
                                    self.configure_vbox(&pipeline, &crop);
                                }
                                None => error!("pipeline is gone"),
                            }
                        }
                    }
                    _ => (),
                }
            }
            MessageView::Error(err) => {
                error!("{}  {}", err.get_error(), self.location);
                self.stop();
            }
            MessageView::Warning(wrn) => {
                warn!("{}  {}", wrn.get_error(), self.location);
                self.stop();
            }
            MessageView::Element(elem) => {
                if let Some(obj) = elem.get_src() {
                    if obj.get_name() == "GstUDPSrcTimeout" {
                        error!("udpsrc timeout -- stopping stream");
                        self.stop();
                    }
                }
            }
            _ => (),
        };
        glib::Continue(true)
    }

    /// Stop the stream
    fn stop(&self) {
        if let Some(control) = &self.control {
            control.stopped();
        }
    }

    /// Configure videobox element
    fn configure_vbox(&self, pipeline: &Pipeline, crop: &MatrixCrop) {
        if let Some(vbx) = pipeline.get_by_name("vbox") {
            match vbx.get_static_pad("src") {
                Some(src_pad) => {
                    match src_pad.get_current_caps() {
                        Some(caps) => {
                            match self.config_vbox_caps(vbx, caps, crop) {
                                Err(e) => error!("setting vbox caps: {}", e),
                                _ => (),
                            }
                        }
                        None => error!("no current caps on vbox src pad"),
                    }
                }
                None => error!("no videobox src pad"),
            }
        }
    }

    /// Configure videobox caps
    fn config_vbox_caps(&self, vbx: Element, caps: Caps, crop: &MatrixCrop)
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
    Ok(ElementFactory::make(factory_name, name)
        .ok_or_else(|| Error::MissingElement(factory_name))?)
}

/// Link a source element with a sink
fn link_src_sink(src: &Element, sink: Element) -> Result<(), Error> {
    src.link(&sink)?;
    let sink = sink.downgrade(); // weak ref
    src.connect_pad_added(move |_src, src_pad| {
        match sink.upgrade() {
            Some(sink) => {
                match sink.get_static_pad("sink") {
                    Some(sink_pad) => {
                        if let Err(e) = src_pad.link(&sink_pad) {
                            error!("link pad: {}", e);
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

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
        self.bus.remove_watch().unwrap();
    }
}

impl Stream {

    /// Set video overlay window handle
    pub fn set_handle(&self, handle: usize) {
        match self.pipeline.get_by_name("sink") {
            Some(sink) => {
                match sink.dynamic_cast::<VideoOverlay>() {
                    Ok(overlay) => unsafe { overlay.set_window_handle(handle) },
                    Err(_) => error!("invalid video overlay"),
                }
            }
            None => error!("no sink element for video overlay"),
        }
    }

    /// Log packet statistics
    pub fn log_stats(&mut self, cam_id: &str) -> bool {
        let pushed = self.pushed;
        let lost = self.lost;
        let late = self.late;
        let update = self.update_stats();
        if update {
            info!("stats {}: {} pushed, {} lost, {} late pkts", cam_id,
                 self.pushed - pushed,
                 self.lost - lost,
                 self.late - late,
            );
        }
        update
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
            Err(e) => warn!("jitter stats: {}", e),
        }
        false
    }

    /// Start the stream
    pub fn start(&self) {
        self.pipeline.set_state(State::Playing).unwrap();
    }

    /// Stop the stream
    pub fn stop(&mut self) {
        self.pipeline.set_state(State::Null).unwrap();
    }

    /// Check if stream has stopped updating
    pub fn check_eos(&mut self) -> Result<(), Error> {
        if let Some(sink) = self.pipeline.get_by_name("sink") {
            self.check_sink(sink)?;
        }
        Ok(())
    }

    /// Check sink to make sure that last-sample is updating.
    ///
    /// If not, post an EOS message on the bus.
    fn check_sink(&mut self, sink: Element) -> Result<(), Error> {
        match sink.get_property("last-sample") {
            Ok(sample) => {
                match sample.get::<Sample>() {
                    Some(sample) => {
                        match sample.get_buffer() {
                            Some(buffer) => {
                                let pts = buffer.get_pts();
                                if pts == self.last_pts {
                                    error!("PTS stuck at {}; posting EOS", pts);
                                    self.bus.post(&Message::new_eos()
                                        .src(Some(&sink)).build())?;
                                }
                                self.last_pts = pts;
                            }
                            None => error!("sample buffer missing"),
                        }
                    }
                    None => error!("last-sample missing"),
                }
            }
            Err(e) => warn!("last-sample: {:?}", e),
        };
        Ok(())
    }
}

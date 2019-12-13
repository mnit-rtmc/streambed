use streambed::*;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::builder().format_timestamp(None).init();
    gstreamer::init().unwrap();
    let stream = StreamBuilder::new(0)
        .with_location("udp://225.69.5.11:5000")
        .with_encoding(Encoding::H264)
        .with_latency(0)
        .with_overlay_text(Some("I-94 WB @ T.H.65"))
        .with_sink(Sink::RTP("225.69.69.69".to_string(), 5000, Encoding::H264,
            false))
        .build()?;
    stream.start();
    let mainloop = glib::MainLoop::new(None, false);
    mainloop.run();
    Ok(())
}

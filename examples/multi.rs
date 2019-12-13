use streambed::*;
use std::env;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::builder().format_timestamp(None).init();
    gstreamer::init().unwrap();
    let mut args = env::args();
    let _prog = args.next();
    let location = args.next();
    let overlay_text = args.next();
    let stream = StreamBuilder::new(0)
        .with_location(&location.expect("Need location"))
        .with_encoding(Encoding::H264)
        .with_latency(200)
        .with_overlay_text(overlay_text.as_ref().map(String::as_ref))
        .with_sink(Sink::RTP("225.69.69.69".to_string(), 5000, Encoding::H264,
            false))
        .build()?;
    stream.start();
    let mainloop = glib::MainLoop::new(None, false);
    mainloop.run();
    Ok(())
}

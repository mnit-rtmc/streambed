use log::info;
use streambed::*;
use std::env;
use std::error::Error;

struct Control { }

impl StreamControl for Control {
    /// Stream started
    fn started(&self) {
        info!("stream started");
    }
    /// Stream stopped
    fn stopped(&self) {
        info!("stream stopped");
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::builder().format_timestamp(None).init();
    gstreamer::init().unwrap();
    let mut args = env::args();
    let _prog = args.next();
    let location = args.next();
    let overlay_text = args.next();
    let _stream = StreamBuilder::new(0)
        .with_source(Source::default()
            .with_location(&location.expect("Need location"))
            .with_encoding(Encoding::H264)
            .with_latency(0))
        .with_overlay_text(overlay_text.as_ref().map(String::as_ref))
        .with_sink(Sink::RTP("226.69.69.69".to_string(), 5000, Encoding::H264,
            false))
        .with_control(Some(Box::new(Control {})))
        .build()?;
    let mainloop = glib::MainLoop::new(None, false);
    mainloop.run();
    Ok(())
}

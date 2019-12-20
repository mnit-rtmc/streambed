// main.rs
//
// Copyright (C) 2019  Minnesota Department of Transportation
//
use clap::{App, Arg, ArgMatches, SubCommand};
use log::info;
use std::env;
use streambed::{
    Acceleration, Encoding, Error, Feedback, Sink, Source, StreamBuilder
};

/// Crate version
const VERSION: &'static str = std::env!("CARGO_PKG_VERSION");

/// Possible video encodings
const ENCODINGS: &[&'static str] = &["MJPEG", "MPEG2", "MPEG4", "H264", "H265",
    "VP8", "VP9"];

struct Control { }

impl Feedback for Control {
    /// Stream playing
    fn playing(&self) {
        info!("stream playing");
    }
    /// Stream stopped
    fn stopped(&self) -> bool {
        info!("stream stopped");
        true
    }
}

/// Create clap App
fn create_app() -> App<'static, 'static> {
    App::new("streambed")
        .version(VERSION)
        .about("Video streaming system")
        .subcommand(SubCommand::with_name("config")
            .about("Configure global settings")
            .display_order(1)
            .arg(Arg::with_name("flows")
                .short("f")
                .long("flows")
                .help("total number of flows")
                .value_name("total"))
            .arg(Arg::with_name("acceleration")
                .short("a")
                .long("accel")
                .help("acceleration method")
                .value_name("method")
                .possible_values(&["none", "vaapi", "omx"])))
        .subcommand(SubCommand::with_name("flow")
            .about("Configure a video flow")
            .display_order(2)
            .arg(Arg::with_name("number")
                .index(1)
                .required(true)
                .help("flow number (starting with 0)")
                .takes_value(true)
            )
            .arg(Arg::with_name("source")
                .short("s")
                .long("source")
                .help("source location or URI")
                .value_name("uri"))
            .arg(Arg::with_name("source-encoding")
                .short("e")
                .long("source-encoding")
                .help("source encoding")
                .value_name("encoding")
                .possible_values(ENCODINGS))
            .arg(Arg::with_name("timeout")
                .short("t")
                .long("timeout")
                .help("source timeout in seconds")
                .value_name("sec"))
            .arg(Arg::with_name("latency")
                .short("l")
                .long("latency")
                .help("buffering latency in milliseconds")
                .value_name("ms"))
            .arg(Arg::with_name("address")
                .short("a")
                .long("address")
                .help("sink UDP address (multicast supported)")
                .value_name("addr"))
            .arg(Arg::with_name("port")
                .short("p")
                .long("port")
                .help("sink UDP port")
                .takes_value(true))
            .arg(Arg::with_name("sink-encoding")
                .short("n")
                .long("sink-encoding")
                .help("sink encoding")
                .value_name("encoding")
                .possible_values(ENCODINGS))
            .arg(Arg::with_name("text")
                .short("x")
                .long("text")
                .help("overlay text (requires transcoding)")
                .takes_value(true)))
        .subcommand(SubCommand::with_name("run")
            .about("Run streambed video system"))
}

/// Run video system
fn run(_matches: &ArgMatches) -> Result<(), Error> {
    gstreamer::init().unwrap();
    let mut args = env::args();
    let _prog = args.next();
    let location = args.next().expect("Need location");
    let overlay_text = args.next();
    let _stream = StreamBuilder::new(0)
        .with_acceleration(Acceleration::VAAPI)
        .with_source(Source::default()
            .with_location(&location)
            .with_encoding(Encoding::H264)
            .with_latency(0))
        .with_overlay_text(overlay_text.as_ref().map(String::as_ref))
        .with_sink(Sink::RTP("226.69.69.69".to_string(), 5000, Encoding::H264,
            true))
        .with_feedback(Some(Box::new(Control {})))
        .build()?;
    let mainloop = glib::MainLoop::new(None, false);
    mainloop.run();
    Ok(())
}

/// Main function
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().format_timestamp(None).init();
    match create_app().get_matches().subcommand() {
        ("run", Some(matches)) => run(matches)?,
        _ => todo!(),
    }
    Ok(())
}

// main.rs
//
// Copyright (C) 2019  Minnesota Department of Transportation
//
use clap::{App, Arg, ArgMatches, SubCommand};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::File;
use streambed::{
    Acceleration, Encoding, Error, Feedback, Sink, Source, FlowBuilder
};

/// Crate version
const VERSION: &'static str = std::env!("CARGO_PKG_VERSION");

/// Configuration file name
const CONFIG_FILE: &'static str = "streambed.muon";

/// Possible video encodings
const ENCODINGS: &[&'static str] = &["MJPEG", "MPEG2", "MPEG4", "H264", "H265",
    "VP8", "VP9"];

/// Streambed configuration
#[derive(Debug, Default, Deserialize, Serialize)]
struct Config {
    acceleration: Option<String>,
    flow: Vec<FlowConfig>,
}

/// Source location
#[derive(Debug, Deserialize, Serialize)]
struct Location(String);

impl Default for Location {
    fn default() -> Self {
        Location { 0: "test".to_string() }
    }
}

/// Configuration for one flow
#[derive(Debug, Default, Deserialize, Serialize)]
struct FlowConfig {
    location: Location,
    encoding: Option<String>,
    timeout: Option<u16>,
    latency: Option<u32>,
    sprops: Option<String>,
    text: Option<String>,
    address: Option<String>,
    port: Option<u16>,
    sink_encoding: Option<String>,
}

struct Control { }

impl Feedback for Control {
    /// Flow playing
    fn playing(&self) {
        info!("flow playing");
    }
    /// Flow stopped
    fn stopped(&self) -> bool {
        info!("flow stopped");
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

impl Config {
    /// Load configuration from file
    fn load() -> Self {
        match File::open(CONFIG_FILE) {
            Ok(muon) => match muon_rs::from_reader(muon) {
                Ok(config) => config,
                Err(_e) => {
                    warn!("Error parsing {}", CONFIG_FILE);
                    Self::default()
                }
            }
            Err(e) => {
                warn!("{:?} error reading {}", e.kind(), CONFIG_FILE);
                Self::default()
            }
        }
    }
}

/// Config sub-command
fn config_subcommand(matches: &ArgMatches) -> Result<(), Error> {
    let mut config = Config::load();
    if let Some(acceleration) = matches.value_of("acceleration") {
        config.acceleration = Some(acceleration.to_string());
    }
    if let Some(flows) = matches.value_of("flows") {
        let flows: usize = flows.parse()?;
        config.flow.resize_with(flows, Default::default);
    }
    print!("{}", muon_rs::to_string(&config)?);
    Ok(())
}

/// Flow sub-command
fn flow_subcommand(_matches: &ArgMatches) -> Result<(), Error> {
    todo!();
}

/// Run sub-command
fn run_subcommand(_matches: &ArgMatches) -> Result<(), Error> {
    gstreamer::init().unwrap();
    let mut args = env::args();
    let _prog = args.next();
    let location = args.next().expect("Need location");
    let overlay_text = args.next();
    let _flow = FlowBuilder::new(0)
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
        ("config", Some(matches)) => config_subcommand(matches)?,
        ("flow", Some(matches)) => flow_subcommand(matches)?,
        ("run", Some(matches)) => run_subcommand(matches)?,
        _ => unreachable!(),
    }
    Ok(())
}

// main.rs
//
// Copyright (C) 2019  Minnesota Department of Transportation
//
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
use log::info;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::str::FromStr;
use streambed::{
    Acceleration, Encoding, Error, Feedback, Sink, Source, FlowBuilder
};

/// Crate version
const VERSION: &'static str = std::env!("CARGO_PKG_VERSION");

/// Configuration file name
const CONFIG_FILE: &'static str = "streambed.muon";

/// Possible video encodings
const ENCODINGS: &[&'static str] = &["", "MJPEG", "MPEG2", "MPEG4", "H264",
    "H265", "VP8", "VP9"];

/// Streambed configuration
#[derive(Debug, Default, Deserialize, Serialize)]
struct Config {
    /// Control port (TCP)
    control_port: Option<u16>,
    /// Video acceleration method
    acceleration: Option<String>,
    /// All flows
    flow: Vec<FlowConfig>,
}

/// Source location (cannot be empty string)
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
    /// Source location URI
    location: Location,
    /// Source encoding
    encoding: Option<String>,
    /// Source timeout in seconds
    timeout: Option<u16>,
    /// Buffering latency in milliseconds
    latency: Option<u32>,
    /// SDP parameter sets
    sprops: Option<String>,
    /// Overlay text
    overlay_text: Option<String>,
    /// Sink address
    address: Option<String>,
    /// Sink port
    port: Option<u16>,
    /// Sink encoding
    sink_encoding: Option<String>,
}

impl FlowConfig {
    /// Get source encoding
    fn encoding(&self) -> Encoding {
        match &self.encoding {
            Some(e) => match e.parse() {
                Ok(e) => e,
                Err(_) => Encoding::default(),
            }
            None => Encoding::default(),
        }
    }
    /// Get source timeout
    fn timeout(&self) -> u16 {
        match self.timeout {
            Some(t) => t,
            None => 2,
        }
    }
    /// Get buffering latency
    fn latency(&self) -> u32 {
        match self.latency {
            Some(l) => l,
            None => 200,
        }
    }
    /// Get source
    fn source(&self) -> Source {
        Source::default()
            .with_location(&self.location.0)
            .with_encoding(self.encoding())
            .with_timeout(self.timeout())
            .with_latency(self.latency())
    }
    /// Get overlay text
    fn overlay_text(&self) -> Option<&str> {
        match &self.overlay_text {
            Some(t) => Some(&t),
            None => None,
        }
    }
    /// Get sink encoding
    fn sink_encoding(&self) -> Encoding {
        match &self.sink_encoding {
            Some(e) => match e.parse() {
                Ok(e) => e,
                Err(_) => self.encoding(),
            }
            None => self.encoding(),
        }
    }
    /// Get sink
    fn sink(&self) -> Sink {
        match (&self.address, &self.port) {
            (Some(address), Some(port)) => {
                Sink::RTP(String::from(address), (*port).into(),
                    self.sink_encoding(), true)
            }
            _ => Sink::FAKE,
        }
    }
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

/// Check if an argument is parseable
fn is_parseable<T: FromStr>(value: String) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    match value.parse::<T>() {
        Ok(_) => Ok(()),
        Err(_) => Err(String::from("Invalid argument")),
    }
}

/// Check if a flows is valid
fn check_flows(flows: usize, value: String) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    match value.parse::<usize>() {
        Ok(f) if f < flows => Ok(()),
        Ok(_) => Err(String::from("Flow index out of bounds")),
        _ => Err(String::from("Invalid argument")),
    }
}

/// Create clap App
fn create_app(config: &Config) -> App<'static, 'static> {
    let flows = config.flow.len();
    App::new("streambed")
        .version(VERSION)
        .setting(AppSettings::GlobalVersion)
        .about("Video streaming system")
        .setting(AppSettings::ArgRequiredElseHelp)
        .subcommand(SubCommand::with_name("config")
            .about("Configure global settings")
            .display_order(1)
            .arg(Arg::with_name("acceleration")
                .short("a")
                .long("acceleration")
                .help("acceleration method")
                .value_name("method")
                .possible_values(&["NONE", "VAAPI", "OMX"]))
            .arg(Arg::with_name("control-port")
                .short("c")
                .long("control-port")
                .help("TCP control port")
                .takes_value(true)
                .validator(is_parseable::<u16>))
            .arg(Arg::with_name("flows")
                .short("f")
                .long("flows")
                .help("total number of flows")
                .value_name("total")
                .validator(is_parseable::<u8>)))
        .subcommand(SubCommand::with_name("flow")
            .about("Configure a video flow")
            .display_order(2)
            .arg(Arg::with_name("number")
                .index(1)
                .required(true)
                .help("flow index number")
                .takes_value(true)
                .validator(move |v| check_flows(flows, v)))
            .arg(Arg::with_name("location")
                .short("u")
                .long("location")
                .help("source location or URI")
                .value_name("uri")
                .empty_values(false))
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
                .value_name("sec")
                .validator(is_parseable::<u16>))
            .arg(Arg::with_name("latency")
                .short("l")
                .long("latency")
                .help("buffering latency in milliseconds")
                .value_name("ms")
                .validator(is_parseable::<u32>))
            .arg(Arg::with_name("address")
                .short("a")
                .long("address")
                .help("sink UDP address (multicast supported)")
                .value_name("addr"))
            .arg(Arg::with_name("port")
                .short("p")
                .long("port")
                .help("sink UDP port")
                .takes_value(true)
                .validator(is_parseable::<u16>))
            .arg(Arg::with_name("sink-encoding")
                .short("n")
                .long("sink-encoding")
                .help("sink encoding")
                .value_name("encoding")
                .possible_values(ENCODINGS))
            .arg(Arg::with_name("overlay-text")
                .short("x")
                .long("overlay text")
                .help("overlay text (requires transcoding)")
                .takes_value(true)))
        .subcommand(SubCommand::with_name("run")
            .about("Run streambed video system"))
}

impl Config {
    /// Load configuration from file
    fn load() -> Self {
        match File::open(CONFIG_FILE) {
            Ok(rdr) => match muon_rs::from_reader(rdr) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("{:?} error parsing {}", e, CONFIG_FILE);
                    Self::default()
                }
            }
            Err(e) => {
                eprintln!("{:?} error reading {}", e.kind(), CONFIG_FILE);
                Self::default()
            }
        }
    }
    /// Store configuration to file
    fn store(&self) {
        match File::create(CONFIG_FILE) {
            Ok(writer) => match muon_rs::to_writer(writer, self) {
                Ok(_) => (),
                Err(_e) => eprintln!("Error storing {}", CONFIG_FILE),
            }
            Err(e) => eprintln!("{:?} error writing {}", e.kind(), CONFIG_FILE),
        }
    }

    /// Config sub-command
    fn config_subcommand(mut self, matches: &ArgMatches) -> Result<(), Error> {
        let mut param = false;
        if let Some(acceleration) = matches.value_of("acceleration") {
            self.acceleration = Some(acceleration.to_string());
            println!("Setting `acceleration` => {}", acceleration);
            param = true;
        }
        if let Some(port) = matches.value_of("control-port") {
            self.control_port = if port.len() > 0 {
                Some(port.parse()?)
            } else {
                None
            };
            println!("Setting `control-port` => {}", port);
            param = true;
        }
        if let Some(flows) = matches.value_of("flows") {
            let flows: usize = flows.parse()?;
            self.flow.resize_with(flows, Default::default);
            println!("Setting `flows` => {}", flows);
            param = true;
        }
        if !param {
            println!("\n{}", muon_rs::to_string(&self)?);
        }
        self.store();
        Ok(())
    }

    /// Flow sub-command
    fn flow_subcommand(mut self, matches: &ArgMatches) -> Result<(), Error> {
        let number = matches.value_of("number").unwrap();
        let number: usize = number.parse()?;
        let mut flow = &mut self.flow[number];
        let mut param = false;
        if let Some(location) = matches.value_of("location") {
            flow.location = Location { 0: String::from(location) };
            println!("Setting `location` => {}", location);
            param = true;
        }
        if let Some(encoding) = matches.value_of("source-encoding") {
            flow.encoding = if encoding.len() > 0 {
                Some(String::from(encoding))
            } else {
                None
            };
            println!("Setting `encoding` => {}", encoding);
            param = true;
        }
        if let Some(timeout) = matches.value_of("timeout") {
            flow.timeout = Some(timeout.parse()?);
            println!("Setting `timeout` => {}", timeout);
            param = true;
        }
        if let Some(latency) = matches.value_of("latency") {
            flow.latency = if latency.len() > 0 {
                Some(latency.parse()?)
            } else {
                None
            };
            println!("Setting `latency` => {}", latency);
            param = true;
        }
        if let Some(address) = matches.value_of("address") {
            flow.address = if address.len() > 0 {
                Some(String::from(address))
            } else {
                None
            };
            println!("Setting `address` => {}", address);
            param = true;
        }
        if let Some(port) = matches.value_of("port") {
            flow.port = if port.len() > 0 {
                Some(port.parse()?)
            } else {
                None
            };
            println!("Setting `port` => {}", port);
            param = true;
        }
        if let Some(encoding) = matches.value_of("sink-encoding") {
            flow.sink_encoding = if encoding.len() > 0 {
                Some(String::from(encoding))
            } else {
                None
            };
            println!("Setting `sink_encoding` => {}", encoding);
            param = true;
        }
        if let Some(text) = matches.value_of("overlay-text") {
            flow.overlay_text = if text.len() > 0 {
                Some(String::from(text))
            } else {
                None
            };
            println!("Setting `overlay-text` => {}", text);
            param = true;
        }
        if !param {
            println!("\n{}", muon_rs::to_string(flow)?);
        }
        self.store();
        Ok(())
    }

    /// Run sub-command
    fn run_subcommand(&self, _matches: &ArgMatches) -> Result<(), Error> {
        if self.flow.len() == 0 {
            eprintln!("No flows defined");
            return Ok(());
        }
        gstreamer::init().unwrap();

        let acceleration = match &self.acceleration {
            Some(a) => a.parse::<Acceleration>()?,
            None => Acceleration::NONE,
        };
        let mut flows = vec![];
        for (i, flow_cfg) in self.flow.iter().enumerate() {
            let flow = FlowBuilder::new(i)
                .with_acceleration(acceleration)
                .with_source(flow_cfg.source())
                .with_overlay_text(flow_cfg.overlay_text())
                .with_sink(flow_cfg.sink())
                .with_feedback(Some(Box::new(Control {})))
                .build()?;
            flows.push(flow);
        }
        let mainloop = glib::MainLoop::new(None, false);
        mainloop.run();
        Ok(())
    }
}

/// Main function
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().format_timestamp(None).init();
    let config = Config::load();
    match create_app(&config).get_matches().subcommand() {
        ("config", Some(matches)) => config.config_subcommand(matches)?,
        ("flow", Some(matches)) => config.flow_subcommand(matches)?,
        ("run", Some(matches)) => config.run_subcommand(matches)?,
        _ => unreachable!(),
    }
    Ok(())
}

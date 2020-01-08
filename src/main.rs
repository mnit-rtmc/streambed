// main.rs
//
// Copyright (C) 2019-2020  Minnesota Department of Transportation
//
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
use env_logger::Env;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, File};
use std::io::{BufRead, BufReader, ErrorKind};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::str::FromStr;
use std::thread;
use streambed::{
    Acceleration, Encoding, Error, Feedback, Flow, FlowBuilder, Sink, Source,
};

/// Crate version
const VERSION: &'static str = std::env!("CARGO_PKG_VERSION");

/// Configuration file name
const CONFIG_FILE: &'static str = "streambed.muon";

/// Possible video encodings
const ENCODINGS: &[&'static str] =
    &["", "MJPEG", "MPEG2", "MPEG4", "H264", "H265", "VP8", "VP9"];

/// ASCII group separator
const SEP_GROUP: u8 = b'\x1D';

/// ASCII record separator
const SEP_RECORD: u8 = b'\x1E';

/// ASCII unit separator
const SEP_UNIT: u8 = b'\x1F';

/// Command parameters
trait Parameters<'a> {
    /// Get the value of a command parameter
    fn value(&'a self, p: &'a str) -> Option<&'a str>;
}

impl<'a> Parameters<'a> for ArgMatches<'a> {
    fn value(&'a self, p: &'a str) -> Option<&'a str> {
        self.value_of(p)
    }
}

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
        Location {
            0: "test".to_string(),
        }
    }
}

/// Configuration for one flow
#[derive(Debug, Default, Deserialize, Serialize)]
struct FlowConfig {
    /// Source location URI
    location: Location,
    /// Source encoding
    source_encoding: Option<String>,
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
    fn source_encoding(&self) -> Encoding {
        match &self.source_encoding {
            Some(e) => match e.parse() {
                Ok(e) => e,
                Err(_) => Encoding::default(),
            },
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
            .with_encoding(self.source_encoding())
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
                Err(_) => self.source_encoding(),
            },
            None => self.source_encoding(),
        }
    }
    /// Get sink
    fn sink(&self) -> Sink {
        match (&self.address, &self.port) {
            (Some(address), Some(port)) => Sink::RTP(
                String::from(address),
                (*port).into(),
                self.sink_encoding(),
                true,
            ),
            _ => Sink::FAKE,
        }
    }
}

/// Control feedback
struct Control {
    /// Flow number
    number: usize,
}

impl Feedback for Control {
    /// Flow playing
    fn playing(&self) {
        debug!("Flow{} feedback playing", self.number);
    }
    /// Flow stopped
    fn stopped(&self) {
        debug!("Flow{} feedback stopped", self.number);
    }
    /// Update statistics
    fn stats(&self, pushed: u64, lost: u64, late: u64) {
        if pushed > 0 || lost > 0 || late > 0 {
            debug!("Flow{} feedback stats: {} pushed, {} lost, {} late",
                self.number, pushed, lost, late);
        }
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
        .subcommand(
            SubCommand::with_name("config")
                .about("Configure global settings")
                .display_order(1)
                .arg(
                    Arg::with_name("acceleration")
                        .short("a")
                        .long("acceleration")
                        .help("acceleration method")
                        .value_name("method")
                        .possible_values(&["NONE", "VAAPI", "OMX"]),
                )
                .arg(
                    Arg::with_name("control-port")
                        .short("c")
                        .long("control-port")
                        .help("TCP control port")
                        .takes_value(true)
                        .validator(is_parseable::<u16>),
                )
                .arg(
                    Arg::with_name("flows")
                        .short("f")
                        .long("flows")
                        .help("total number of flows")
                        .value_name("total")
                        .validator(is_parseable::<u8>),
                ),
        )
        .subcommand(
            SubCommand::with_name("flow")
                .about("Configure a video flow")
                .display_order(2)
                .arg(
                    Arg::with_name("number")
                        .index(1)
                        .required(true)
                        .help("flow index number")
                        .takes_value(true)
                        .validator(move |v| check_flows(flows, v)),
                )
                .arg(
                    Arg::with_name("location")
                        .short("u")
                        .long("location")
                        .help("source location or URI")
                        .value_name("uri")
                        .empty_values(false),
                )
                .arg(
                    Arg::with_name("source-encoding")
                        .short("e")
                        .long("source-encoding")
                        .help("source encoding")
                        .value_name("encoding")
                        .possible_values(ENCODINGS),
                )
                .arg(
                    Arg::with_name("timeout")
                        .short("t")
                        .long("timeout")
                        .help("source timeout in seconds")
                        .value_name("sec")
                        .validator(is_parseable::<u16>),
                )
                .arg(
                    Arg::with_name("latency")
                        .short("l")
                        .long("latency")
                        .help("buffering latency in milliseconds")
                        .value_name("ms")
                        .validator(is_parseable::<u32>),
                )
                .arg(
                    Arg::with_name("address")
                        .short("a")
                        .long("address")
                        .help("sink UDP address (multicast supported)")
                        .value_name("addr"),
                )
                .arg(
                    Arg::with_name("port")
                        .short("p")
                        .long("port")
                        .help("sink UDP port")
                        .takes_value(true)
                        .validator(is_parseable::<u16>),
                )
                .arg(
                    Arg::with_name("sink-encoding")
                        .short("n")
                        .long("sink-encoding")
                        .help("sink encoding")
                        .value_name("encoding")
                        .possible_values(ENCODINGS),
                )
                .arg(
                    Arg::with_name("overlay-text")
                        .short("x")
                        .long("overlay-text")
                        .help("overlay text (requires transcoding)")
                        .takes_value(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("run").about("Run streambed video system"),
        )
}

macro_rules! set_param {
    ($num:expr, $param:expr) => {
        info!(
            concat!("Setting flow{} `", stringify!($param), "` => {}",),
            $num, $param,
        );
    };
}

impl Config {
    /// Get config file path
    fn path() -> PathBuf {
        let mut path = dirs::config_dir().expect("No config directory");
        path.push("streambed");
        path.push(CONFIG_FILE);
        path
    }
    /// Load configuration from file
    fn load() -> Self {
        let path = Config::path();
        match File::open(&path) {
            Ok(rdr) => match muon_rs::from_reader(rdr) {
                Ok(config) => config,
                Err(e) => {
                    error!("{:?} parsing {:?}", e, path);
                    panic!("Invalid configuration");
                }
            },
            Err(e) => {
                error!("{:?} reading {:?}", e.kind(), path);
                if e.kind() != ErrorKind::NotFound {
                    panic!("Invalid configuration");
                }
                Self::default()
            }
        }
    }
    /// Store configuration to file
    fn store(&self) {
        let path = Config::path();
        if !path.exists() {
            if let Err(e) = create_dir_all(&path.parent().unwrap()) {
                error!("{:?} creating {:?}", e.kind(), path);
            }
        }
        match File::create(&path) {
            Ok(writer) => {
                if let Err(_e) = muon_rs::to_writer(writer, self) {
                    error!("storing {:?}", path);
                }
            }
            Err(e) => error!("{:?} writing {:?}", e.kind(), path),
        }
    }

    /// Config sub-command
    fn config_subcommand<'a, P: Parameters<'a>>(
        &mut self, params: &'a P,
    ) -> Result<(), Error> {
        let mut param = false;
        if let Some(acceleration) = params.value("acceleration") {
            self.acceleration = Some(acceleration.to_string());
            info!("Setting `acceleration` => {}", acceleration);
            param = true;
        }
        if let Some(port) = params.value("control-port") {
            self.control_port = if port.len() > 0 {
                Some(port.parse()?)
            } else {
                None
            };
            info!("Setting `control-port` => {}", port);
            param = true;
        }
        if let Some(flows) = params.value("flows") {
            let flows: usize = flows.parse()?;
            self.flow.resize_with(flows, Default::default);
            info!("Setting `flows` => {}", flows);
            param = true;
        }
        if !param {
            println!("\n{}", muon_rs::to_string(&self)?);
        }
        self.store();
        Ok(())
    }

    /// Flow sub-command
    fn flow_subcommand<'a, P: Parameters<'a>>(
        &mut self, params: &'a P,
    ) -> Result<usize, Error> {
        let number = params
            .value("number")
            .ok_or(Error::Other("Missing flow number"))?;
        let number: usize = number.parse()?;
        if number >= self.flow.len() {
            return Err(Error::Other("Invalid flow number"));
        }
        let mut flow = &mut self.flow[number];
        let mut param = false;
        if let Some(location) = params.value("location") {
            flow.location = Location {
                0: String::from(location),
            };
            set_param!(number, location);
            param = true;
        }
        if let Some(source_encoding) = params.value("source-encoding") {
            flow.source_encoding = if source_encoding.len() > 0 {
                Some(String::from(source_encoding))
            } else {
                None
            };
            set_param!(number, source_encoding);
            param = true;
        }
        if let Some(timeout) = params.value("timeout") {
            flow.timeout = Some(timeout.parse()?);
            set_param!(number, timeout);
            param = true;
        }
        if let Some(latency) = params.value("latency") {
            flow.latency = if latency.len() > 0 {
                Some(latency.parse()?)
            } else {
                None
            };
            set_param!(number, latency);
            param = true;
        }
        if let Some(address) = params.value("address") {
            flow.address = if address.len() > 0 {
                Some(String::from(address))
            } else {
                None
            };
            set_param!(number, address);
            param = true;
        }
        if let Some(port) = params.value("port") {
            flow.port = if port.len() > 0 {
                Some(port.parse()?)
            } else {
                None
            };
            set_param!(number, port);
            param = true;
        }
        if let Some(sink_encoding) = params.value("sink-encoding") {
            flow.sink_encoding = if sink_encoding.len() > 0 {
                Some(String::from(sink_encoding))
            } else {
                None
            };
            set_param!(number, sink_encoding);
            param = true;
        }
        if let Some(overlay_text) = params.value("overlay-text") {
            flow.overlay_text = if overlay_text.len() > 0 {
                Some(String::from(overlay_text))
            } else {
                None
            };
            set_param!(number, overlay_text);
            param = true;
        }
        if !param {
            println!("\n{}", muon_rs::to_string(flow)?);
        }
        self.store();
        Ok(number)
    }
    /// Convert config into a Vec of Flows
    fn into_flows(self) -> Result<Vec<Flow>, Error> {
        let mut flows = vec![];
        for i in 0..self.flow.len() {
            flows.push(self.flow(i)?);
        }
        Ok(flows)
    }
    /// Create a flow
    fn flow(&self, i: usize) -> Result<Flow, Error> {
        let acceleration = match &self.acceleration {
            Some(a) => a.parse::<Acceleration>()?,
            None => Acceleration::NONE,
        };
        if let Some(flow_cfg) = self.flow.iter().skip(i).next() {
            let flow = FlowBuilder::new(i)
                .with_acceleration(acceleration)
                .with_source(flow_cfg.source())
                .with_overlay_text(flow_cfg.overlay_text())
                .with_sink(flow_cfg.sink())
                .with_feedback(Some(Box::new(Control { number: i })))
                .build()?;
            Ok(flow)
        } else {
            Err(Error::Other("Invalid flow number"))
        }
    }
}

/// Main function
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env = Env::default().default_filter_or("info");
    env_logger::from_env(env).format_timestamp(None).init();
    let mut config = Config::load();
    match create_app(&config).get_matches().subcommand() {
        ("config", Some(matches)) => config.config_subcommand(matches)?,
        ("flow", Some(matches)) => {
            config.flow_subcommand(matches)?;
        }
        ("run", Some(_matches)) => {
            gstreamer::init().expect("gstreamer init failed!");
            let control_port = config.control_port.unwrap_or(8001);
            let flows = config.into_flows()?;
            run_subcommand(control_port, flows)?
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Run sub-command
fn run_subcommand(control_port: u16, flows: Vec<Flow>) -> Result<(), Error> {
    let address: IpAddr = "::".parse()?;
    let listener = TcpListener::bind((address, control_port))?;
    thread::spawn(move || command_thread(listener, flows));
    let mainloop = glib::MainLoop::new(None, false);
    mainloop.run();
    Ok(())
}

/// Thread to handle remote commands
fn command_thread(
    listener: TcpListener, mut flows: Vec<Flow>,
) -> Result<(), Error> {
    loop {
        let (socket, remote) = listener.accept()?;
        info!("command connection OPENED: {:?}", remote);
        match process_commands(socket, &mut flows) {
            Err(e) => warn!("{:?} processing command", e),
            _ => (),
        }
        info!("command connection CLOSED: {:?}", remote);
    }
}

/// Process remote commands
fn process_commands(
    socket: TcpStream, flows: &mut Vec<Flow>,
) -> Result<(), Error> {
    let mut buf = vec![];
    let mut reader = BufReader::new(socket);
    loop {
        let n_bytes = reader.read_until(SEP_GROUP, &mut buf)?;
        if n_bytes == 0 {
            break;
        }
        match buf.pop() {
            Some(SEP_GROUP) => {
                let cmd = std::str::from_utf8(&buf)?;
                process_command(cmd, flows)?;
            }
            Some(b) => {
                debug!("Invalid command separator: 0x{:X}", b);
                return Err(Error::Other("Invalid command separator"));
            }
            None => break,
        }
        buf.clear();
    }
    Ok(())
}

/// Process a remote command
fn process_command(cmd: &str, flows: &mut Vec<Flow>) -> Result<(), Error> {
    // Maybe someday, use SEP_RECORD instead of \x1E
    if cmd.starts_with("flow\x1E") {
        let params = &cmd[5..];
        let mut config = Config::load();
        let number = config.flow_subcommand(&params)?;
        flows[number] = config.flow(number)?;
        return Ok(());
    } else if cmd.starts_with("config\x1E") {
        let params = &cmd[7..];
        let mut config = Config::load();
        config.config_subcommand(&params)?;
        flows.clear();
        flows.extend(config.into_flows()?);
        return Ok(());
    }
    debug!("Invalid command: {:?}", cmd);
    Err(Error::Other("Invalid command"))
}

impl<'a> Parameters<'a> for &'a str {
    fn value(&'a self, key: &'a str) -> Option<&'a str> {
        self.split(char::from(SEP_RECORD))
            .find_map(|p| param_value(p, key))
    }
}

/// Get the value of one parameter
fn param_value<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    let mut p = params.split(char::from(SEP_UNIT));
    if let Some(k) = p.next() {
        if k == key {
            if let (Some(value), None) = (p.next(), p.next()) {
                return Some(value);
            }
        }
    }
    None
}

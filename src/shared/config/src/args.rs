use crate::configs::{load_config, Config};
use anyhow::{anyhow, Result};
use clap::{error::ErrorKind, parser::ValueSource, Arg, ArgMatches, Command};
use std::ffi::OsString;

#[derive(Debug, Clone)]
pub struct StreamerArgs {
    pub file: String,
    pub address: String,
    pub port: u16,
    pub interface: Option<String>,
    pub ptp_domain: Option<u8>,
    pub config_file: Option<String>,
    pub verbose: bool,
    pub duration_seconds: Option<f64>,
    pub loop_playback: bool,
    pub gain_db: f32,
    pub ttl: u8,
    pub sap: bool,
    pub sdp_output: Option<String>,
    pub payload_type: u8,
    pub ssrc: Option<u32>,
    pub session_name: String,
    pub packet_time_ms: u32,
}

pub type Args = StreamerArgs;

#[derive(Debug, Clone)]
pub enum StreamerCommand {
    File(StreamerArgs),
    MusicPlayer(MusicPlayerArgs),
}

#[derive(Debug, Clone)]
pub struct MusicPlayerArgs {
    pub address: String,
    pub port: u16,
    pub interface: Option<String>,
    pub ptp_domain: u8,
    pub verbose: bool,
    pub ttl: u8,
    pub sap: bool,
    pub payload_type: u8,
    pub session_name: String,
    pub packet_time_ms: u32,
}

#[derive(Debug, Clone)]
pub struct PlayerArgs {
    pub sdp: Option<String>,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub interface: Option<String>,
    pub sender: Option<String>,
    pub channels: Option<u16>,
    pub payload_type: Option<u8>,
    pub latency_ms: u32,
    pub output_device: Option<String>,
    pub duration_seconds: Option<f64>,
    pub verbose: bool,
    pub list_devices: bool,
    pub test_null_output: bool,
}

#[derive(Debug, Clone)]
pub struct SapArgs {
    pub interface: String,
    pub once: bool,
    pub sdp_output_dir: Option<String>,
    pub verbose: bool,
    pub listen_address: String,
    pub port: u16,
}

pub fn parse_args() -> Result<StreamerArgs> {
    parse_streamer_args()
}

pub fn parse_args_from<I, T>(args: I) -> Result<StreamerArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    parse_streamer_args_from(args)
}

pub fn parse_streamer_args() -> Result<StreamerArgs> {
    parse_streamer_args_from(std::env::args_os())
}

pub fn parse_streamer_command() -> Result<StreamerCommand> {
    parse_streamer_command_from(std::env::args_os())
}

pub fn parse_player_args() -> Result<PlayerArgs> {
    parse_player_args_from(std::env::args_os())
}

pub fn parse_sap_args() -> Result<SapArgs> {
    parse_sap_args_from(std::env::args_os())
}

pub fn is_display_control_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<clap::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        )
    })
}

pub fn parse_streamer_args_from<I, T>(args: I) -> Result<StreamerArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = streamer_cli().try_get_matches_from(args)?;
    streamer_args_from_matches(&matches)
}

pub fn parse_streamer_command_from<I, T>(args: I) -> Result<StreamerCommand>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = streamer_cli().try_get_matches_from(args)?;
    streamer_command_from_matches(&matches)
}

pub fn parse_player_args_from<I, T>(args: I) -> Result<PlayerArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = player_cli().try_get_matches_from(args)?;
    player_args_from_matches(&matches)
}

pub fn parse_sap_args_from<I, T>(args: I) -> Result<SapArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = sap_cli().try_get_matches_from(args)?;
    sap_args_from_matches(&matches)
}

fn streamer_cli() -> Command {
    Command::new("aes67-streamer")
        .version(env!("AES67_TOOLS_VERSION"))
        .author("Jiajun Yang")
        .about("Cross-platform CLI tool for streaming audio files over RTP networks with AES67 compliance")
        .args_conflicts_with_subcommands(true)
        .subcommand(music_player_cli())
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .value_name("FILE")
                .help("Audio file to stream (WAV, FLAC, MP3, AIFF); required unless supplied by config")
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .value_name("IP")
                .help("Multicast IP address; required unless supplied by config")
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("UDP port number")
                .default_value("5004")
                .value_parser(clap::value_parser!(u16))
        )
        .arg(
            Arg::new("interface")
                .short('i')
                .long("interface")
                .value_name("INTERFACE")
                .help("Network interface name or IPv4 address [default: 127.0.0.1]")
        )
        .arg(
            Arg::new("sdp-output")
                .long("sdp-output")
                .value_name("FILE")
                .help("Write the generated SDP description to this file before streaming")
        )
        .arg(
            Arg::new("ptp-domain")
                .long("ptp-domain")
                .value_name("DOMAIN")
                .help("PTP domain number (0-255) [default: 0]")
                .value_parser(clap::value_parser!(u8))
        )
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Configuration file path (TOML format) [default: none]")
        )
        .arg(
            Arg::new("duration-seconds")
                .long("duration-seconds")
                .value_name("SECONDS")
                .help("Stop streaming after this many seconds [default: unlimited]")
                .value_parser(parse_positive_duration_seconds)
        )
        .arg(
            Arg::new("loop")
                .long("loop")
                .help("Loop the audio file instead of stopping at end-of-file [default: false]")
                .action(clap::ArgAction::SetTrue)
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Enable verbose logging [default: false]")
                .action(clap::ArgAction::SetTrue)
        )
}

fn music_player_cli() -> Command {
    Command::new("music-player")
        .about("Open a terminal music player that streams a playlist over AES67")
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .value_name("IP")
                .help("Multicast IP address for the AES67 RTP stream")
                .required(true),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("UDP port number")
                .default_value("5004")
                .value_parser(clap::value_parser!(u16)),
        )
        .arg(
            Arg::new("interface")
                .short('i')
                .long("interface")
                .value_name("INTERFACE")
                .help("Network interface name or IPv4 address [default: 127.0.0.1]"),
        )
        .arg(
            Arg::new("ptp-domain")
                .long("ptp-domain")
                .value_name("DOMAIN")
                .help("PTP domain number (0-255) [default: 0]")
                .default_value("0")
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            Arg::new("session-name")
                .long("session-name")
                .value_name("NAME")
                .help("SAP/SDP session name for the music player stream")
                .default_value("AES67 Music Player"),
        )
        .arg(
            Arg::new("packet-time-ms")
                .long("packet-time-ms")
                .value_name("MS")
                .help("RTP packet time in milliseconds [default: 1]")
                .default_value("1")
                .value_parser(parse_positive_u32),
        )
        .arg(
            Arg::new("payload-type")
                .long("payload-type")
                .value_name("PT")
                .help("Dynamic RTP payload type for L24 audio [default: 97]")
                .default_value("97")
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            Arg::new("ttl")
                .long("ttl")
                .value_name("HOPS")
                .help("Multicast TTL for RTP and SAP packets [default: 32]")
                .default_value("32")
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            Arg::new("no-sap")
                .long("no-sap")
                .help("Disable SAP announcement for the music player stream")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Enable verbose logging [default: false]")
                .action(clap::ArgAction::SetTrue),
        )
}

fn player_cli() -> Command {
    Command::new("aes67-player")
        .version(env!("AES67_TOOLS_VERSION"))
        .author("Jiajun Yang")
        .about("CLI tool for receiving and playing AES67 RTP audio streams")
        .arg(
            Arg::new("sdp")
                .long("sdp")
                .value_name("FILE")
                .help("SDP file describing the AES67 stream"),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .value_name("IP")
                .help("RTP destination address to receive in basic CLI mode"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("RTP UDP port to receive in basic CLI mode")
                .value_parser(clap::value_parser!(u16)),
        )
        .arg(
            Arg::new("interface")
                .short('i')
                .long("interface")
                .value_name("INTERFACE")
                .help("Network interface name or IPv4 address"),
        )
        .arg(
            Arg::new("sender")
                .long("sender")
                .value_name("IP")
                .help("Optional sender IPv4 address filter"),
        )
        .arg(
            Arg::new("channels")
                .long("channels")
                .value_name("COUNT")
                .help("Audio channel count for basic CLI mode [default: 2]")
                .value_parser(clap::value_parser!(u16)),
        )
        .arg(
            Arg::new("payload-type")
                .long("payload-type")
                .value_name("PT")
                .help("RTP payload type for basic CLI mode [default: 97]")
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            Arg::new("latency-ms")
                .long("latency-ms")
                .value_name("MS")
                .help("Initial playout latency in milliseconds")
                .default_value("50")
                .value_parser(parse_positive_u32),
        )
        .arg(
            Arg::new("output-device")
                .short('o')
                .long("output-device")
                .value_name("DEVICE")
                .help("CPAL output device index from --list-devices or device name"),
        )
        .arg(
            Arg::new("list-devices")
                .short('L')
                .long("list-devices")
                .help("List audio output devices and exit")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("test-null-output")
                .long("test-null-output")
                .help("Use an internal null output sink for automated tests")
                .hide(true)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("duration-seconds")
                .long("duration-seconds")
                .value_name("SECONDS")
                .help("Stop receiving after this many seconds [default: unlimited]")
                .value_parser(parse_positive_duration_seconds),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Enable verbose logging [default: false]")
                .action(clap::ArgAction::SetTrue),
        )
}

fn sap_cli() -> Command {
    Command::new("aes67-sap")
        .version(env!("AES67_TOOLS_VERSION"))
        .author("Jiajun Yang")
        .about("Browse AES67 streams announced with SAP")
        .arg(
            Arg::new("interface")
                .short('i')
                .long("interface")
                .value_name("INTERFACE")
                .help("Network interface name or IPv4 address used for SAP multicast")
                .required(true),
        )
        .arg(
            Arg::new("once")
                .long("once")
                .help("Exit after the first discovered AES67 SAP stream")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("sdp-output-dir")
                .long("sdp-output-dir")
                .value_name("DIR")
                .help("Write each discovered SDP payload to this directory"),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Enable verbose logging [default: false]")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("test-address")
                .long("test-address")
                .value_name("IP")
                .help("Override SAP listen address for automated tests")
                .hide(true)
                .default_value("239.255.255.255"),
        )
        .arg(
            Arg::new("test-port")
                .long("test-port")
                .value_name("PORT")
                .help("Override SAP listen port for automated tests")
                .hide(true)
                .default_value("9875")
                .value_parser(clap::value_parser!(u16)),
        )
}

fn streamer_command_from_matches(matches: &ArgMatches) -> Result<StreamerCommand> {
    match matches.subcommand() {
        Some(("music-player", subcommand)) => {
            music_player_args_from_matches(subcommand).map(StreamerCommand::MusicPlayer)
        }
        Some((name, _)) => Err(anyhow!("unknown aes67-streamer command: {name}")),
        None => streamer_args_from_matches(matches).map(StreamerCommand::File),
    }
}

fn streamer_args_from_matches(matches: &ArgMatches) -> Result<StreamerArgs> {
    let config_file = matches.get_one::<String>("config").cloned();
    let config = match config_file.as_deref() {
        Some(path) => Some(load_config(path)?),
        None => None,
    };

    let file = cli_string(matches, "file")
        .or_else(|| config.as_ref().and_then(|config| config.audio.file.clone()))
        .ok_or_else(|| missing_required_value("audio file", "--file", "audio.file"))?;

    let address = cli_string(matches, "address")
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.stream.address.clone())
        })
        .ok_or_else(|| {
            missing_required_value("multicast address", "--address", "stream.address")
        })?;

    Ok(StreamerArgs {
        file,
        address,
        port: merged_port(matches, config.as_ref()),
        interface: cli_string(matches, "interface").or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.stream.interface.clone())
        }),
        ptp_domain: cli_u8(matches, "ptp-domain")
            .or_else(|| config.as_ref().and_then(|config| config.stream.ptp_domain)),
        config_file,
        verbose: matches.get_flag("verbose") || merged_verbose(config.as_ref()),
        duration_seconds: matches
            .get_one::<f64>("duration-seconds")
            .copied()
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.audio.duration_seconds)
            }),
        loop_playback: merged_loop_playback(matches, config.as_ref()),
        gain_db: merged_gain_db(config.as_ref()),
        ttl: merged_ttl(config.as_ref())?,
        sap: merged_sap(config.as_ref()),
        sdp_output: cli_string(matches, "sdp-output").or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.stream.sdp_output.clone())
        }),
        payload_type: merged_payload_type(config.as_ref())?,
        ssrc: merged_ssrc(config.as_ref()),
        session_name: merged_session_name(config.as_ref()),
        packet_time_ms: merged_packet_time_ms(config.as_ref())?,
    })
}

fn music_player_args_from_matches(matches: &ArgMatches) -> Result<MusicPlayerArgs> {
    let ttl = *matches
        .get_one::<u8>("ttl")
        .expect("clap should supply music-player TTL default");
    if ttl == 0 {
        return Err(anyhow!("--ttl must be greater than zero"));
    }

    let payload_type = validate_l24_payload_type(
        *matches
            .get_one::<u8>("payload-type")
            .expect("clap should supply music-player payload type default"),
        "--payload-type",
    )?;

    Ok(MusicPlayerArgs {
        address: matches
            .get_one::<String>("address")
            .expect("clap should require music-player address")
            .clone(),
        port: *matches
            .get_one::<u16>("port")
            .expect("clap should supply music-player port default"),
        interface: cli_string(matches, "interface"),
        ptp_domain: *matches
            .get_one::<u8>("ptp-domain")
            .expect("clap should supply music-player PTP domain default"),
        verbose: matches.get_flag("verbose"),
        ttl,
        sap: !matches.get_flag("no-sap"),
        payload_type,
        session_name: matches
            .get_one::<String>("session-name")
            .expect("clap should supply music-player session name default")
            .clone(),
        packet_time_ms: *matches
            .get_one::<u32>("packet-time-ms")
            .expect("clap should supply music-player packet time default"),
    })
}

fn player_args_from_matches(matches: &ArgMatches) -> Result<PlayerArgs> {
    let list_devices = matches.get_flag("list-devices");
    let sdp = cli_string(matches, "sdp");
    let address = cli_string(matches, "address");
    let port = cli_u16(matches, "port");
    let channels = cli_u16(matches, "channels");
    let payload_type = cli_u8(matches, "payload-type");

    if list_devices {
        // Device listing is a local audio query and does not need receive stream metadata.
    } else if sdp.is_some() {
        let conflicting_args = [
            ("address", address.is_some()),
            ("port", port.is_some()),
            ("channels", channels.is_some()),
            ("payload-type", payload_type.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect::<Vec<_>>();

        if !conflicting_args.is_empty() {
            return Err(anyhow!(
                "--sdp cannot be combined with stream-format arguments: {}",
                conflicting_args.join(", ")
            ));
        }
    } else {
        if address.is_none() {
            return Err(missing_required_value(
                "receive address",
                "--address",
                "player.receive.address",
            ));
        }
        if port.is_none() {
            return Err(missing_required_value(
                "receive port",
                "--port",
                "player.receive.port",
            ));
        }
    }

    let channels = match channels {
        Some(channels) if !(1..=8).contains(&channels) => {
            return Err(anyhow!("--channels must be between 1 and 8"));
        }
        Some(channels) => Some(channels),
        None if sdp.is_none() => Some(2),
        None => None,
    };

    let payload_type = match payload_type {
        Some(payload_type) => Some(validate_l24_payload_type(payload_type, "--payload-type")?),
        None if sdp.is_none() => Some(97),
        None => None,
    };

    Ok(PlayerArgs {
        sdp,
        address,
        port,
        interface: cli_string(matches, "interface"),
        sender: cli_string(matches, "sender"),
        channels,
        payload_type,
        latency_ms: *matches
            .get_one::<u32>("latency-ms")
            .expect("clap should supply latency default"),
        output_device: cli_string(matches, "output-device"),
        duration_seconds: matches.get_one::<f64>("duration-seconds").copied(),
        verbose: matches.get_flag("verbose"),
        list_devices,
        test_null_output: matches.get_flag("test-null-output"),
    })
}

fn sap_args_from_matches(matches: &ArgMatches) -> Result<SapArgs> {
    Ok(SapArgs {
        interface: cli_string(matches, "interface")
            .expect("clap should require SAP browser interface"),
        once: matches.get_flag("once"),
        sdp_output_dir: cli_string(matches, "sdp-output-dir"),
        verbose: matches.get_flag("verbose"),
        listen_address: matches
            .get_one::<String>("test-address")
            .expect("clap should supply SAP listen address default")
            .clone(),
        port: *matches
            .get_one::<u16>("test-port")
            .expect("clap should supply SAP listen port default"),
    })
}

fn cli_string(matches: &ArgMatches, id: &str) -> Option<String> {
    if matches.value_source(id) == Some(ValueSource::CommandLine) {
        matches.get_one::<String>(id).cloned()
    } else {
        None
    }
}

fn cli_u8(matches: &ArgMatches, id: &str) -> Option<u8> {
    if matches.value_source(id) == Some(ValueSource::CommandLine) {
        matches.get_one::<u8>(id).copied()
    } else {
        None
    }
}

fn cli_u16(matches: &ArgMatches, id: &str) -> Option<u16> {
    if matches.value_source(id) == Some(ValueSource::CommandLine) {
        matches.get_one::<u16>(id).copied()
    } else {
        None
    }
}

fn merged_port(matches: &ArgMatches, config: Option<&Config>) -> u16 {
    if matches.value_source("port") == Some(ValueSource::CommandLine) {
        return *matches
            .get_one::<u16>("port")
            .expect("clap should parse command line port");
    }

    config
        .and_then(|config| config.stream.port)
        .or_else(|| matches.get_one::<u16>("port").copied())
        .unwrap_or(5004)
}

fn merged_verbose(config: Option<&Config>) -> bool {
    config
        .and_then(|config| config.runtime.verbose)
        .unwrap_or(false)
}

fn merged_loop_playback(matches: &ArgMatches, config: Option<&Config>) -> bool {
    if matches.value_source("loop") == Some(ValueSource::CommandLine) {
        return true;
    }

    config
        .and_then(|config| config.audio.loop_playback)
        .unwrap_or(false)
}

fn merged_gain_db(config: Option<&Config>) -> f32 {
    config
        .and_then(|config| config.audio.gain_db)
        .unwrap_or(0.0)
}

fn merged_ttl(config: Option<&Config>) -> Result<u8> {
    let ttl = config.and_then(|config| config.stream.ttl).unwrap_or(32);

    if ttl == 0 {
        return Err(anyhow!("stream.ttl must be greater than zero"));
    }

    Ok(ttl)
}

fn merged_sap(config: Option<&Config>) -> bool {
    config.and_then(|config| config.stream.sap).unwrap_or(true)
}

fn merged_payload_type(config: Option<&Config>) -> Result<u8> {
    let payload_type = config
        .and_then(|config| config.stream.payload_type)
        .unwrap_or(97);

    validate_l24_payload_type(payload_type, "stream.payload_type")?;

    Ok(payload_type)
}

fn validate_l24_payload_type(payload_type: u8, name: &str) -> Result<u8> {
    if !(96..=127).contains(&payload_type) {
        return Err(anyhow!(
            "{name} must be a dynamic RTP payload type between 96 and 127 for L24"
        ));
    }

    Ok(payload_type)
}

fn merged_ssrc(config: Option<&Config>) -> Option<u32> {
    config.and_then(|config| config.stream.ssrc)
}

fn merged_session_name(config: Option<&Config>) -> String {
    config
        .and_then(|config| config.stream.name.clone())
        .unwrap_or_else(|| "AES67 Stream".to_string())
}

fn merged_packet_time_ms(config: Option<&Config>) -> Result<u32> {
    let Some(packet_time_ms) = config.and_then(|config| config.stream.packet_time_ms) else {
        return Ok(1);
    };

    if packet_time_ms == 0 {
        return Err(anyhow!("stream.packet_time_ms must be greater than zero"));
    }

    Ok(packet_time_ms)
}

fn missing_required_value(name: &str, cli_flag: &str, config_key: &str) -> anyhow::Error {
    anyhow!("missing {name}; pass {cli_flag} or set {config_key} in the config file")
}

fn parse_positive_duration_seconds(value: &str) -> Result<f64, String> {
    let duration = value
        .parse::<f64>()
        .map_err(|_| "duration must be a number of seconds".to_string())?;

    if duration.is_finite() && duration > 0.0 {
        Ok(duration)
    } else {
        Err("duration must be greater than zero".to_string())
    }
}

fn parse_positive_u32(value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| "value must be a positive integer".to_string())?;

    if parsed > 0 {
        Ok(parsed)
    } else {
        Err("value must be greater than zero".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_CONFIG_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn write_temp_config(contents: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let sequence = TEMP_CONFIG_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "aes67-streamer-config-test-{}-{id}-{sequence}.toml",
            std::process::id()
        ));
        fs::write(&path, contents).expect("temp config should be writable");
        path
    }

    #[test]
    fn test_args_structure() {
        // Test that the Args structure compiles correctly
        let args = Args {
            file: "test.wav".to_string(),
            address: "239.192.1.1".to_string(),
            port: 5004,
            interface: Some("eth0".to_string()),
            ptp_domain: Some(0),
            config_file: None,
            verbose: false,
            duration_seconds: None,
            loop_playback: false,
            gain_db: 0.0,
            ttl: 32,
            sap: true,
            sdp_output: None,
            payload_type: 97,
            ssrc: None,
            session_name: "AES67 Stream".to_string(),
            packet_time_ms: 1,
        };

        assert_eq!(args.file, "test.wav");
        assert_eq!(args.address, "239.192.1.1");
        assert_eq!(args.port, 5004);
        assert!(!args.loop_playback);
    }

    #[test]
    fn test_duration_seconds_parsed() {
        let args = parse_args_from([
            "aes67-streamer",
            "--file",
            "test.wav",
            "--address",
            "239.192.1.1",
            "--duration-seconds",
            "1.5",
        ])
        .expect("duration should parse");

        assert_eq!(args.duration_seconds, Some(1.5));
        assert!(!args.loop_playback);
    }

    #[test]
    fn test_duration_seconds_must_be_positive() {
        let result = parse_args_from([
            "aes67-streamer",
            "--file",
            "test.wav",
            "--address",
            "239.192.1.1",
            "--duration-seconds",
            "0",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn config_file_supplies_required_runtime_values() {
        let path = write_temp_config(
            r#"
                [audio]
                file = "configured.wav"
                loop = true

                [stream]
                address = "239.10.20.30"
                port = 6000
                interface = "127.0.0.1"
                ptp_domain = 7
            "#,
        );

        let args = parse_args_from([
            "aes67-streamer",
            "--config",
            path.to_str().expect("temp path should be utf-8"),
        ])
        .expect("config file should supply required values");

        assert_eq!(args.file, "configured.wav");
        assert_eq!(args.address, "239.10.20.30");
        assert_eq!(args.port, 6000);
        assert_eq!(args.interface.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.ptp_domain, Some(7));
        assert_eq!(args.ssrc, None);
        assert!(args.loop_playback);

        fs::remove_file(path).ok();
    }

    #[test]
    fn cli_values_override_config_file_values() {
        let path = write_temp_config(
            r#"
                [audio]
                file = "configured.wav"
                loop = false

                [stream]
                address = "239.10.20.30"
                port = 6000
                interface = "lo0"
                ptp_domain = 7
            "#,
        );

        let args = parse_args_from([
            "aes67-streamer",
            "--config",
            path.to_str().expect("temp path should be utf-8"),
            "--file",
            "cli.wav",
            "--address",
            "239.30.20.10",
            "--port",
            "7000",
            "--interface",
            "127.0.0.1",
            "--ptp-domain",
            "9",
        ])
        .expect("cli values should override config values");

        assert_eq!(args.file, "cli.wav");
        assert_eq!(args.address, "239.30.20.10");
        assert_eq!(args.port, 7000);
        assert_eq!(args.interface.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.ptp_domain, Some(9));
        assert!(!args.loop_playback);

        fs::remove_file(path).ok();
    }

    #[test]
    fn missing_file_after_merge_is_an_error() {
        let result = parse_args_from(["aes67-streamer", "--address", "239.192.1.1"]);

        assert!(result.is_err());
    }

    #[test]
    fn invalid_config_path_is_an_error_even_with_cli_values() {
        let missing_path = std::env::temp_dir().join(format!(
            "aes67-streamer-config-test-missing-{}.toml",
            std::process::id()
        ));
        fs::remove_file(&missing_path).ok();

        let result = parse_args_from([
            "aes67-streamer",
            "--config",
            missing_path
                .to_str()
                .expect("missing temp path should be utf-8"),
            "--file",
            "test.wav",
            "--address",
            "239.192.1.1",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn loop_playback_cli_flag_enables_looping() {
        let args = parse_args_from([
            "aes67-streamer",
            "--file",
            "test.wav",
            "--address",
            "239.192.1.1",
            "--loop",
        ])
        .expect("loop flag should parse");

        assert!(args.loop_playback);
    }

    #[test]
    fn config_file_supplies_stream_metadata() {
        let path = write_temp_config(
            r#"
                [audio]
                file = "configured.wav"
                gain_db = -6.0

                [stream]
                address = "239.10.20.30"
                ttl = 12
                payload_type = 101
                ssrc = 3735928559
                name = "Configured Stream"
                packet_time_ms = 2
            "#,
        );

        let args = parse_args_from([
            "aes67-streamer",
            "--config",
            path.to_str().expect("temp path should be utf-8"),
        ])
        .expect("config file should supply stream metadata");

        assert_eq!(args.ttl, 12);
        assert_eq!(args.payload_type, 101);
        assert_eq!(args.ssrc, Some(3735928559));
        assert_eq!(args.session_name, "Configured Stream");
        assert_eq!(args.packet_time_ms, 2);
        assert_eq!(args.gain_db, -6.0);

        fs::remove_file(path).ok();
    }

    #[test]
    fn new_config_layout_supplies_audio_stream_and_runtime_values() {
        let path = write_temp_config(
            r#"
                [audio]
                file = "configured.wav"
                loop = true
                duration_seconds = 1.5
                gain_db = -6.0

                [stream]
                name = "Configured Stream"
                address = "239.10.20.30"
                port = 6000
                interface = "127.0.0.1"
                sdp_output = "configured.sdp"
                packet_time_ms = 2
                payload_type = 101
                ssrc = 3735928559
                ttl = 12
                sap = false
                ptp_domain = 7

                [runtime]
                verbose = true
            "#,
        );

        let args = parse_args_from([
            "aes67-streamer",
            "--config",
            path.to_str().expect("temp path should be utf-8"),
        ])
        .expect("new config layout should supply runtime values");

        assert_eq!(args.file, "configured.wav");
        assert_eq!(args.address, "239.10.20.30");
        assert_eq!(args.port, 6000);
        assert_eq!(args.interface.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.sdp_output.as_deref(), Some("configured.sdp"));
        assert_eq!(args.ptp_domain, Some(7));
        assert_eq!(args.duration_seconds, Some(1.5));
        assert!(args.loop_playback);
        assert!(args.verbose);
        assert_eq!(args.ttl, 12);
        assert_eq!(args.payload_type, 101);
        assert_eq!(args.ssrc, Some(3735928559));
        assert_eq!(args.session_name, "Configured Stream");
        assert_eq!(args.packet_time_ms, 2);
        assert_eq!(args.gain_db, -6.0);
        assert!(!args.sap);

        fs::remove_file(path).ok();
    }

    #[test]
    fn invalid_config_metadata_is_an_error() {
        let path = write_temp_config(
            r#"
                [audio]
                file = "configured.wav"

                [stream]
                address = "239.10.20.30"
                ttl = 0
                payload_type = 128
            "#,
        );

        let config = load_config(path.to_str().expect("temp path should be utf-8"))
            .expect("config should parse before validation");
        assert_eq!(config.stream.ttl, Some(0));
        assert_eq!(config.stream.payload_type, Some(128));

        let result = parse_args_from([
            "aes67-streamer",
            "--config",
            path.to_str().expect("temp path should be utf-8"),
        ]);

        assert!(result.is_err());

        fs::remove_file(path).ok();
    }

    #[test]
    fn streamer_config_rejects_static_payload_type_for_l24() {
        let path = write_temp_config(
            r#"
                [audio]
                file = "configured.wav"

                [stream]
                address = "239.10.20.30"
                payload_type = 95
            "#,
        );

        let result = parse_args_from([
            "aes67-streamer",
            "--config",
            path.to_str().expect("temp path should be utf-8"),
        ]);

        assert!(result.is_err());

        fs::remove_file(path).ok();
    }

    #[test]
    fn parse_streamer_args_from_keeps_existing_streamer_cli_behavior() {
        let args = parse_streamer_args_from([
            "aes67-streamer",
            "--file",
            "test.wav",
            "--address",
            "239.192.1.1",
        ])
        .expect("streamer args should parse");

        assert_eq!(args.file, "test.wav");
        assert_eq!(args.address, "239.192.1.1");
        assert_eq!(args.port, 5004);
    }

    #[test]
    fn parse_streamer_command_from_keeps_file_mode_without_subcommand() {
        let command = parse_streamer_command_from([
            "aes67-streamer",
            "--file",
            "track.wav",
            "--address",
            "239.69.83.1",
            "--port",
            "5004",
        ])
        .expect("existing file mode should still parse");

        let StreamerCommand::File(args) = command else {
            panic!("root args should remain file mode");
        };

        assert_eq!(args.file, "track.wav");
        assert_eq!(args.address, "239.69.83.1");
        assert_eq!(args.port, 5004);
    }

    #[test]
    fn parse_streamer_command_from_accepts_music_player_subcommand() {
        let command = parse_streamer_command_from([
            "aes67-streamer",
            "music-player",
            "--address",
            "239.69.83.1",
            "--interface",
            "127.0.0.1",
        ])
        .expect("music-player mode should parse");

        let StreamerCommand::MusicPlayer(args) = command else {
            panic!("music-player subcommand should select music-player mode");
        };

        assert_eq!(args.address, "239.69.83.1");
        assert_eq!(args.port, 5004);
        assert_eq!(args.interface.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn streamer_cli_accepts_sdp_output_path() {
        let args = parse_streamer_args_from([
            "aes67-streamer",
            "--file",
            "test.wav",
            "--address",
            "239.192.1.1",
            "--sdp-output",
            "stream.sdp",
        ])
        .expect("streamer args should parse");

        assert_eq!(args.sdp_output.as_deref(), Some("stream.sdp"));
    }

    #[test]
    fn player_basic_cli_supplies_receive_format_defaults() {
        let args = parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--interface",
            "127.0.0.1",
            "--sender",
            "127.0.0.1",
            "--duration-seconds",
            "1.5",
            "--verbose",
        ])
        .expect("player basic cli should parse");

        assert_eq!(args.sdp, None);
        assert_eq!(args.address.as_deref(), Some("239.192.1.1"));
        assert_eq!(args.port, Some(5004));
        assert_eq!(args.interface.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.sender.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.channels, Some(2));
        assert_eq!(args.payload_type, Some(97));
        assert_eq!(args.latency_ms, 50);
        assert_eq!(args.duration_seconds, Some(1.5));
        assert!(args.verbose);
        assert!(!args.test_null_output);
    }

    #[test]
    fn player_basic_cli_accepts_explicit_receive_format() {
        let args = parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--channels",
            "8",
            "--payload-type",
            "101",
            "--latency-ms",
            "75",
            "-o",
            "Studio Monitor",
        ])
        .expect("explicit player format should parse");

        assert_eq!(args.channels, Some(8));
        assert_eq!(args.payload_type, Some(101));
        assert_eq!(args.latency_ms, 75);
        assert_eq!(args.output_device.as_deref(), Some("Studio Monitor"));
    }

    #[test]
    fn player_basic_cli_rejects_static_payload_type_for_l24() {
        let result = parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--payload-type",
            "95",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn player_cli_requires_sdp_or_basic_address_and_port() {
        assert!(parse_player_args_from(["aes67-player"]).is_err());
        assert!(parse_player_args_from(["aes67-player", "--address", "239.192.1.1"]).is_err());
        assert!(parse_player_args_from(["aes67-player", "--port", "5004"]).is_err());
    }

    #[test]
    fn player_cli_list_devices_does_not_require_stream_args() {
        let long_args = parse_player_args_from(["aes67-player", "--list-devices"])
            .expect("device listing should not require stream args");
        assert!(long_args.list_devices);
        assert_eq!(long_args.address, None);
        assert_eq!(long_args.port, None);

        let short_args = parse_player_args_from(["aes67-player", "-L"])
            .expect("short device listing should not require stream args");
        assert!(short_args.list_devices);
    }

    #[test]
    fn player_sdp_mode_keeps_runtime_args() {
        let args = parse_player_args_from([
            "aes67-player",
            "--sdp",
            "tests/example.sdp",
            "--interface",
            "127.0.0.1",
            "--sender",
            "127.0.0.1",
            "--latency-ms",
            "100",
        ])
        .expect("sdp mode should parse with runtime args");

        assert_eq!(args.sdp.as_deref(), Some("tests/example.sdp"));
        assert_eq!(args.address, None);
        assert_eq!(args.port, None);
        assert_eq!(args.channels, None);
        assert_eq!(args.payload_type, None);
        assert_eq!(args.interface.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.sender.as_deref(), Some("127.0.0.1"));
        assert_eq!(args.latency_ms, 100);
    }

    #[test]
    fn player_sdp_mode_rejects_stream_format_overrides() {
        for args in [
            [
                "aes67-player",
                "--sdp",
                "stream.sdp",
                "--address",
                "239.1.1.1",
            ],
            ["aes67-player", "--sdp", "stream.sdp", "--port", "5004"],
            ["aes67-player", "--sdp", "stream.sdp", "--channels", "2"],
            [
                "aes67-player",
                "--sdp",
                "stream.sdp",
                "--payload-type",
                "97",
            ],
        ] {
            assert!(parse_player_args_from(args).is_err());
        }
    }

    #[test]
    fn player_cli_validates_receive_format() {
        assert!(parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--channels",
            "0"
        ])
        .is_err());
        assert!(parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--channels",
            "9"
        ])
        .is_err());
        assert!(parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--payload-type",
            "128"
        ])
        .is_err());
        assert!(parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--latency-ms",
            "0"
        ])
        .is_err());
    }

    #[test]
    fn player_cli_accepts_hidden_test_null_output() {
        let args = parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--test-null-output",
        ])
        .expect("hidden test output should parse");

        assert!(args.test_null_output);
    }

    #[test]
    fn player_cli_does_not_expose_output_backend_choice() {
        assert!(parse_player_args_from([
            "aes67-player",
            "--address",
            "239.192.1.1",
            "--port",
            "5004",
            "--output",
            "alsa"
        ])
        .is_err());
    }

    #[test]
    fn sap_cli_requires_interface() {
        assert!(parse_sap_args_from(["aes67-sap"]).is_err());
    }

    #[test]
    fn sap_cli_defaults_to_continuous_multicast_browse() {
        let args = parse_sap_args_from(["aes67-sap", "--interface", "127.0.0.1"])
            .expect("SAP browser args should parse");

        assert_eq!(args.interface, "127.0.0.1");
        assert!(!args.once);
        assert_eq!(args.sdp_output_dir, None);
        assert!(!args.verbose);
        assert_eq!(args.listen_address, "239.255.255.255");
        assert_eq!(args.port, 9875);
    }

    #[test]
    fn sap_cli_accepts_once_sdp_output_dir_and_verbose() {
        let args = parse_sap_args_from([
            "aes67-sap",
            "--interface",
            "en0",
            "--once",
            "--sdp-output-dir",
            "discovered",
            "--verbose",
        ])
        .expect("SAP browser args should parse");

        assert_eq!(args.interface, "en0");
        assert!(args.once);
        assert_eq!(args.sdp_output_dir.as_deref(), Some("discovered"));
        assert!(args.verbose);
    }

    #[test]
    fn sap_cli_accepts_hidden_listen_override_for_tests() {
        let args = parse_sap_args_from([
            "aes67-sap",
            "--interface",
            "127.0.0.1",
            "--test-address",
            "127.0.0.1",
            "--test-port",
            "19000",
        ])
        .expect("SAP browser test args should parse");

        assert_eq!(args.listen_address, "127.0.0.1");
        assert_eq!(args.port, 19000);
    }
}

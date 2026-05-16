use crate::configs::{load_config, Config};
use anyhow::{anyhow, Result};
use clap::{error::ErrorKind, parser::ValueSource, Arg, ArgMatches, Command};
use std::ffi::OsString;

#[derive(Debug, Clone)]
pub struct Args {
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
    pub payload_type: u8,
    pub ssrc: Option<u32>,
    pub session_name: String,
    pub packet_time_ms: u32,
}

pub fn parse_args() -> Result<Args> {
    parse_args_from(std::env::args_os())
}

pub fn is_display_control_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<clap::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
        )
    })
}

pub fn parse_args_from<I, T>(args: I) -> Result<Args>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = cli().try_get_matches_from(args)?;
    args_from_matches(&matches)
}

fn cli() -> Command {
    Command::new("aes67-streamer")
        .version("0.1.0")
        .author("Jiajun Yang")
        .about("Cross-platform CLI tool for streaming audio files over RTP networks with AES67 compliance")
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

fn args_from_matches(matches: &ArgMatches) -> Result<Args> {
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

    Ok(Args {
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
        payload_type: merged_payload_type(config.as_ref())?,
        ssrc: merged_ssrc(config.as_ref()),
        session_name: merged_session_name(config.as_ref()),
        packet_time_ms: merged_packet_time_ms(config.as_ref())?,
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

    if payload_type > 127 {
        return Err(anyhow!("stream.payload_type must be between 0 and 127"));
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
}

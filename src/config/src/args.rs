use crate::configs::{load_config, Config};
use anyhow::{anyhow, Result};
use clap::{parser::ValueSource, Arg, ArgMatches, Command};
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
}

pub fn parse_args() -> Result<Args> {
    parse_args_from(std::env::args_os())
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
                .help("Audio file to stream (WAV, MP3, AIFF); required unless supplied by config")
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
                .help("Network interface name (e.g., eth0, wlan0)")
        )
        .arg(
            Arg::new("ptp-domain")
                .long("ptp-domain")
                .value_name("DOMAIN")
                .help("PTP domain number (0-255)")
                .value_parser(clap::value_parser!(u8))
        )
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Configuration file path (TOML format)")
        )
        .arg(
            Arg::new("duration-seconds")
                .long("duration-seconds")
                .value_name("SECONDS")
                .help("Stop streaming after this many seconds")
                .value_parser(parse_positive_duration_seconds)
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Enable verbose logging")
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
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.audio.file_path.clone())
        })
        .ok_or_else(|| missing_required_value("audio file", "--file", "audio.file_path"))?;

    let address = cli_string(matches, "address")
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.multicast_address.clone())
        })
        .ok_or_else(|| {
            missing_required_value(
                "multicast address",
                "--address",
                "network.multicast_address",
            )
        })?;

    Ok(Args {
        file,
        address,
        port: merged_port(matches, config.as_ref()),
        interface: cli_string(matches, "interface").or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.interface.clone())
        }),
        ptp_domain: cli_u8(matches, "ptp-domain")
            .or_else(|| config.as_ref().and_then(|config| config.ptp.domain)),
        config_file,
        verbose: matches.get_flag("verbose"),
        duration_seconds: matches.get_one::<f64>("duration-seconds").copied(),
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
        .and_then(|config| config.network.port)
        .or_else(|| matches.get_one::<u16>("port").copied())
        .unwrap_or(5004)
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_temp_config(contents: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aes67-streamer-config-test-{}-{id}.toml",
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
        };

        assert_eq!(args.file, "test.wav");
        assert_eq!(args.address, "239.192.1.1");
        assert_eq!(args.port, 5004);
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
                file_path = "configured.wav"

                [network]
                multicast_address = "239.10.20.30"
                port = 6000
                interface = "127.0.0.1"

                [ptp]
                domain = 7
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

        fs::remove_file(path).ok();
    }

    #[test]
    fn cli_values_override_config_file_values() {
        let path = write_temp_config(
            r#"
                [audio]
                file_path = "configured.wav"

                [network]
                multicast_address = "239.10.20.30"
                port = 6000
                interface = "lo0"

                [ptp]
                domain = 7
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
}

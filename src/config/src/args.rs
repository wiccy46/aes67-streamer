use clap::{Arg, ArgMatches, Command};
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

pub fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    parse_args_from(std::env::args_os())
}

pub fn parse_args_from<I, T>(args: I) -> Result<Args, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = cli().try_get_matches_from(args)?;
    Ok(args_from_matches(&matches))
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
                .help("Audio file to stream (WAV, MP3, AIFF)")
                .required(true)
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .value_name("IP")
                .help("Multicast IP address")
                .required(true)
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

fn args_from_matches(matches: &ArgMatches) -> Args {
    Args {
        file: matches.get_one::<String>("file").unwrap().clone(),
        address: matches.get_one::<String>("address").unwrap().clone(),
        port: *matches.get_one::<u16>("port").unwrap(),
        interface: matches.get_one::<String>("interface").cloned(),
        ptp_domain: matches.get_one::<u8>("ptp-domain").copied(),
        config_file: matches.get_one::<String>("config").cloned(),
        verbose: matches.get_flag("verbose"),
        duration_seconds: matches.get_one::<f64>("duration-seconds").copied(),
    }
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
}

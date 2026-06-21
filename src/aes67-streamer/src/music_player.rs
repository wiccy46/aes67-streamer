use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

const CONFIG_DIR_ENV: &str = "AES67_MUSIC_PLAYER_CONFIG_DIR";
const SETTINGS_FILE: &str = "music-player.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MusicPlayerSettings {
    stream: StreamSettings,
    playlist: PlaylistSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StreamSettings {
    address: String,
    port: u16,
    interface: Option<String>,
    session_name: String,
    sap: bool,
    ptp_domain: u8,
    payload_type: u8,
    packet_time_ms: u32,
    ttl: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct PlaylistSettings {
    files: Vec<String>,
}

impl Default for MusicPlayerSettings {
    fn default() -> Self {
        Self {
            stream: StreamSettings {
                address: "239.69.83.1".to_string(),
                port: 5004,
                interface: None,
                session_name: "AES67 Music Player".to_string(),
                sap: true,
                ptp_domain: 0,
                payload_type: 97,
                packet_time_ms: 1,
                ttl: 32,
            },
            playlist: PlaylistSettings::default(),
        }
    }
}

pub fn run() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_with_io(stdin.lock(), stdout.lock())
}

fn run_with_io<R, W>(mut input: R, mut output: W) -> Result<()>
where
    R: BufRead,
    W: Write,
{
    let settings_path = settings_file_path()?;
    let mut settings = load_or_create_settings(&settings_path)?;

    write_screen(&mut output, &settings, &settings_path, "Ready")?;

    let mut line = String::new();
    loop {
        write!(output, "> ")?;
        output.flush()?;

        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }

        let command = line.trim();
        if matches!(command, "q" | "quit") {
            break;
        }

        let status = apply_command(command, &mut settings, &settings_path)?;
        write_screen(&mut output, &settings, &settings_path, &status)?;
    }

    Ok(())
}

fn apply_command(
    command: &str,
    settings: &mut MusicPlayerSettings,
    settings_path: &Path,
) -> Result<String> {
    if command.is_empty() {
        return Ok("Ready".to_string());
    }

    let Some((name, value)) = command.split_once(' ') else {
        return match command {
            "w" | "write" | "save" => {
                save_settings(settings_path, settings)?;
                Ok("Saved settings".to_string())
            }
            "sap" => {
                settings.stream.sap = !settings.stream.sap;
                save_settings(settings_path, settings)?;
                Ok("Saved SAP setting".to_string())
            }
            _ => Ok("Unknown command".to_string()),
        };
    };

    match name {
        "a" | "address" => {
            settings.stream.address = value.trim().to_string();
            save_settings(settings_path, settings)?;
            Ok("Saved stream address".to_string())
        }
        "p" | "port" => {
            settings.stream.port = parse_u16(value, "port")?;
            save_settings(settings_path, settings)?;
            Ok("Saved stream port".to_string())
        }
        "i" | "interface" => {
            let value = value.trim();
            settings.stream.interface = if matches!(value, "none" | "clear" | "-") {
                None
            } else {
                Some(value.to_string())
            };
            save_settings(settings_path, settings)?;
            Ok("Saved interface".to_string())
        }
        "n" | "name" | "session-name" => {
            settings.stream.session_name = value.trim().to_string();
            save_settings(settings_path, settings)?;
            Ok("Saved session name".to_string())
        }
        "domain" | "ptp-domain" => {
            settings.stream.ptp_domain = parse_u8(value, "PTP domain")?;
            save_settings(settings_path, settings)?;
            Ok("Saved PTP domain".to_string())
        }
        "payload" | "payload-type" => {
            let payload_type = parse_u8(value, "payload type")?;
            if !(96..=127).contains(&payload_type) {
                return Err(anyhow!("payload type must be between 96 and 127"));
            }
            settings.stream.payload_type = payload_type;
            save_settings(settings_path, settings)?;
            Ok("Saved payload type".to_string())
        }
        "ptime" | "packet-time-ms" => {
            settings.stream.packet_time_ms = parse_positive_u32(value, "packet time")?;
            save_settings(settings_path, settings)?;
            Ok("Saved packet time".to_string())
        }
        "ttl" => {
            settings.stream.ttl = parse_u8(value, "TTL")?;
            if settings.stream.ttl == 0 {
                return Err(anyhow!("TTL must be greater than zero"));
            }
            save_settings(settings_path, settings)?;
            Ok("Saved TTL".to_string())
        }
        _ => Ok("Unknown command".to_string()),
    }
}

fn write_screen(
    output: &mut impl Write,
    settings: &MusicPlayerSettings,
    settings_path: &Path,
    status: &str,
) -> Result<()> {
    writeln!(output, "\x1b[2J\x1b[H")?;
    writeln!(output, "AES67 Music Player")?;
    writeln!(output, "===================")?;
    writeln!(output)?;
    writeln!(output, "Stream")?;
    writeln!(output, "  Address: {}", settings.stream.address)?;
    writeln!(output, "  Port: {}", settings.stream.port)?;
    writeln!(
        output,
        "  Interface: {}",
        settings.stream.interface.as_deref().unwrap_or("(not set)")
    )?;
    writeln!(output, "  Session: {}", settings.stream.session_name)?;
    writeln!(
        output,
        "  SAP: {}",
        if settings.stream.sap {
            "enabled"
        } else {
            "disabled"
        }
    )?;
    writeln!(output, "  PTP domain: {}", settings.stream.ptp_domain)?;
    writeln!(output, "  Payload type: {}", settings.stream.payload_type)?;
    writeln!(
        output,
        "  Packet time: {} ms",
        settings.stream.packet_time_ms
    )?;
    writeln!(output, "  TTL: {}", settings.stream.ttl)?;
    writeln!(output)?;
    writeln!(output, "Playlist")?;
    writeln!(output, "  Items: {}", settings.playlist.files.len())?;
    writeln!(output)?;
    writeln!(output, "Settings: {}", settings_path.display())?;
    writeln!(output, "Status: {status}")?;
    writeln!(output)?;
    writeln!(
        output,
        "Commands: a ADDRESS | p PORT | i INTERFACE | sap | domain N | q"
    )?;
    Ok(())
}

fn settings_file_path() -> Result<PathBuf> {
    Ok(settings_dir()?.join(SETTINGS_FILE))
}

fn settings_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(path));
    }

    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("aes67-tools"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(config_home).join("aes67-tools"));
        }

        let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
        Ok(PathBuf::from(home).join(".config").join("aes67-tools"))
    }
}

fn load_or_create_settings(path: &Path) -> Result<MusicPlayerSettings> {
    if path.exists() {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read settings file {}", path.display()))?;
        return toml::from_str(&contents)
            .with_context(|| format!("failed to parse settings file {}", path.display()));
    }

    let settings = MusicPlayerSettings::default();
    save_settings(path, &settings)?;
    Ok(settings)
}

fn save_settings(path: &Path, settings: &MusicPlayerSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create settings directory {}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(settings)?;
    fs::write(path, contents)
        .with_context(|| format!("failed to write settings file {}", path.display()))
}

fn parse_u8(value: &str, name: &str) -> Result<u8> {
    value
        .trim()
        .parse::<u8>()
        .with_context(|| format!("{name} must be an integer between 0 and 255"))
}

fn parse_u16(value: &str, name: &str) -> Result<u16> {
    value
        .trim()
        .parse::<u16>()
        .with_context(|| format!("{name} must be an integer between 0 and 65535"))
}

fn parse_positive_u32(value: &str, name: &str) -> Result<u32> {
    let parsed = value
        .trim()
        .parse::<u32>()
        .with_context(|| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(anyhow!("{name} must be greater than zero"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings_file(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("aes67-music-player-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join(SETTINGS_FILE)
    }

    #[test]
    fn first_run_persists_default_settings() {
        let path = temp_settings_file("first-run");

        let settings = load_or_create_settings(&path).expect("settings should load");

        assert_eq!(settings, MusicPlayerSettings::default());
        assert!(fs::read_to_string(&path)
            .expect("settings should be readable")
            .contains("address"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn command_updates_and_persists_stream_address() {
        let path = temp_settings_file("command-address");
        let mut settings = load_or_create_settings(&path).expect("settings should load");

        let status = apply_command("address 239.69.83.9", &mut settings, &path)
            .expect("command should apply");

        assert_eq!(status, "Saved stream address");
        assert_eq!(settings.stream.address, "239.69.83.9");
        assert!(fs::read_to_string(&path)
            .expect("settings should be readable")
            .contains("239.69.83.9"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }
}

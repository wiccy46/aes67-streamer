use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub stream: StreamConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub file: Option<String>,
    #[serde(rename = "loop")]
    pub loop_playback: Option<bool>,
    pub duration_seconds: Option<f64>,
    pub gain_db: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    pub name: Option<String>,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub interface: Option<String>,
    pub packet_time_ms: Option<u32>,
    pub payload_type: Option<u8>,
    pub ssrc: Option<u32>,
    pub ttl: Option<u8>,
    pub sap: Option<bool>,
    pub ptp_domain: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub verbose: Option<bool>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            file: None,
            loop_playback: Some(false),
            duration_seconds: None,
            gain_db: Some(0.0),
        }
    }
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            name: Some("AES67 Stream".to_string()),
            address: Some("239.192.1.1".to_string()),
            port: Some(5004),
            interface: None,
            packet_time_ms: Some(1),
            payload_type: Some(97),
            ssrc: None,
            ttl: Some(32),
            sap: Some(true),
            ptp_domain: Some(0),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            verbose: Some(false),
        }
    }
}

pub fn load_config(path: &str) -> Result<Config> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read config file {path}"))?;
    let config: Config =
        toml::from_str(&content).with_context(|| format!("failed to parse config file {path}"))?;
    Ok(config)
}

pub fn create_default_config() -> Config {
    Config::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.stream.port, Some(5004));
        assert_eq!(config.stream.payload_type, Some(97));
        assert_eq!(config.stream.ssrc, None);
        assert_eq!(config.runtime.verbose, Some(false));
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("loop"));
        assert!(toml_str.contains("address"));
        assert!(toml_str.contains("verbose"));
    }
}

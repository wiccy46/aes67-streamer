use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub rtp: RtpConfig,
    #[serde(default)]
    pub ptp: PtpConfig,
    #[serde(default)]
    pub performance: PerformanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub file_path: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bit_depth: Option<u8>,
    pub loop_playback: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub multicast_address: Option<String>,
    pub port: Option<u16>,
    pub interface: Option<String>,
    pub ttl: Option<u8>,
    pub buffer_size: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtpConfig {
    pub payload_type: Option<u8>,
    pub ssrc: Option<u32>,
    pub session_name: Option<String>,
    pub packet_time_us: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtpConfig {
    pub domain: Option<u8>,
    pub priority1: Option<u8>,
    pub priority2: Option<u8>,
    pub announce_interval_ms: Option<u32>,
    pub sync_interval_ms: Option<u32>,
    pub clock_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    pub enable_rt_scheduling: Option<bool>,
    pub audio_thread_priority: Option<String>,
    pub network_thread_priority: Option<String>,
    pub buffer_size_ms: Option<u32>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            file_path: None,
            sample_rate: Some(48000),
            channels: Some(2),
            bit_depth: Some(24),
            loop_playback: Some(false),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            multicast_address: Some("239.192.1.1".to_string()),
            port: Some(5004),
            interface: None,
            ttl: Some(32),
            buffer_size: Some(65536),
        }
    }
}

impl Default for RtpConfig {
    fn default() -> Self {
        Self {
            payload_type: Some(97),
            ssrc: Some(0x12345678),
            session_name: Some("AES67 Stream".to_string()),
            packet_time_us: Some(1000),
        }
    }
}

impl Default for PtpConfig {
    fn default() -> Self {
        Self {
            domain: Some(0),
            priority1: Some(128),
            priority2: Some(128),
            announce_interval_ms: Some(1000),
            sync_interval_ms: Some(125),
            clock_source: Some("auto".to_string()),
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            enable_rt_scheduling: Some(true),
            audio_thread_priority: Some("high".to_string()),
            network_thread_priority: Some("high".to_string()),
            buffer_size_ms: Some(15),
        }
    }
}


pub fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
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
        assert_eq!(config.audio.sample_rate, Some(48000));
        assert_eq!(config.network.port, Some(5004));
        assert_eq!(config.rtp.payload_type, Some(97));
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("sample_rate"));
        assert!(toml_str.contains("multicast_address"));
    }
}
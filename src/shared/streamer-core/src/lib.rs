pub mod engine;
pub mod source;

pub use engine::{Aes67Streamer, StreamConfig};
pub use source::{SilenceSource, StreamAudioSource};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_config_default_matches_existing_release_defaults() {
        let config = StreamConfig::default();

        assert_eq!(config.target_sample_rate, 48_000);
        assert_eq!(config.packet_time_ms, 1);
        assert_eq!(config.payload_type, 97);
        assert!(config.sap);
    }
}

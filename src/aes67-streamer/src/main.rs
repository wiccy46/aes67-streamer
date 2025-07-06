use std::process;

mod streamer;
use streamer::{Aes67Streamer, StreamConfig};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("Starting AES67 Audio Streamer");

    // Parse CLI arguments
    let args = match config::parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error parsing arguments: {}", e);
            process::exit(1);
        }
    };

    log::info!("Parsed arguments: {:?}", args);

    log::info!("Audio file: {}", args.file);
    log::info!("Multicast address: {}", args.address);
    log::info!("Port: {}", args.port);

    if let Some(interface) = &args.interface {
        log::info!("Network interface: {}", interface);
    }

    // Create streaming configuration
    let stream_config = StreamConfig {
        sample_rate: args.sample_rate.unwrap_or(48000),
        packet_time_ms: 1, // 1ms packets for AES67
        gain_db: 0.0, // Unity gain for now
        ptp_domain: args.ptp_domain.unwrap_or(0),
        enable_src: true,
        src_quality: audio::ResamplerQuality::Medium,
        verbose: args.verbose,
        enable_threading: true,
    };

    // Create and start streamer
    let mut streamer = match Aes67Streamer::new(
        &args.file,
        &args.address,
        args.port,
        args.interface.as_deref(),
        stream_config,
    ) {
        Ok(streamer) => streamer,
        Err(e) => {
            log::error!("Failed to create streamer: {}", e);
            process::exit(1);
        }
    };

    // Start streaming
    if let Err(e) = streamer.start() {
        log::error!("Streaming failed: {}", e);
        process::exit(1);
    }

    log::info!("AES67 streaming completed successfully")
}

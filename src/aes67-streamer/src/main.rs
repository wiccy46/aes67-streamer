use std::process;
use std::time::Duration;

use config::{StreamerArgs, StreamerCommand};

mod music_player;
mod runtime;
use runtime::RuntimeSupervisor;
use streamer_core::{Aes67Streamer, StreamConfig};

#[tokio::main]
async fn main() {
    let command = match config::parse_streamer_command() {
        Ok(command) => command,
        Err(e) => {
            if config::is_display_control_error(&e) {
                print!("{e}");
                process::exit(0);
            }
            eprintln!("Error parsing arguments: {e}");
            process::exit(1);
        }
    };

    match command {
        StreamerCommand::File(args) => run_file_streamer(args).await,
        StreamerCommand::MusicPlayer(_args) => {
            if let Err(error) = music_player::run().await {
                eprintln!("Error: {error:#}");
                process::exit(1);
            }
        }
    }
}

async fn run_file_streamer(args: StreamerArgs) {
    let default_level = if args.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
        .init();

    log::info!("Starting AES67 Audio Streamer");
    log::debug!("Parsed arguments: {args:?}");
    log::info!("Audio file: {}", args.file);
    log::info!("Multicast address: {}", args.address);
    log::info!("Port: {}", args.port);

    if let Some(interface) = &args.interface {
        log::info!("Network interface: {interface}");
    }

    let stream_config = StreamConfig {
        target_sample_rate: 48000, // AES67 always requires 48kHz
        packet_time_ms: args.packet_time_ms,
        gain_db: args.gain_db,
        ptp_domain: args.ptp_domain.unwrap_or(0),
        verbose: args.verbose,
        duration: args.duration_seconds.map(Duration::from_secs_f64),
        loop_playback: args.loop_playback,
        ttl: args.ttl,
        sap: args.sap,
        payload_type: args.payload_type,
        ssrc: args.ssrc,
        session_name: args.session_name,
    };

    // Create and start streamer
    let mut streamer = match Aes67Streamer::new(
        &args.file,
        &args.address,
        args.port,
        args.interface.as_deref(),
        stream_config,
    )
    .await
    {
        Ok(streamer) => streamer,
        Err(e) => {
            log::error!("Failed to create streamer: {e}");
            process::exit(1);
        }
    };

    if let Some(path) = args.sdp_output.as_deref() {
        if let Err(e) = streamer.write_sdp_file(path) {
            log::error!("Failed to write SDP file: {e:#}");
            process::exit(1);
        }
        log::info!("Wrote SDP file: {path}");
    }

    let supervisor = RuntimeSupervisor::new();
    if let Err(e) = supervisor.run_streamer(&mut streamer).await {
        log::error!("Streaming failed: {:?}", e);
        process::exit(1);
    }

    log::info!("AES67 streaming completed successfully")
}

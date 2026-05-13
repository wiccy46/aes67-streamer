use std::process;
use std::time::Duration;

mod streamer;
use streamer::{Aes67Streamer, StreamConfig};

#[tokio::main]
async fn main() {
    let args = match config::parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error parsing arguments: {e}");
            process::exit(1);
        }
    };

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
        packet_time_ms: 1,         // 1ms packets for AES67
        gain_db: 0.0,              // Unity gain for now
        ptp_domain: args.ptp_domain.unwrap_or(0),
        verbose: args.verbose,
        duration: args.duration_seconds.map(Duration::from_secs_f64),
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

    if let Err(e) = streamer.start_until_shutdown(shutdown_signal()).await {
        log::error!("Streaming failed: {:?}", e);
        process::exit(1);
    }

    log::info!("AES67 streaming completed successfully")
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let interrupt = tokio::signal::ctrl_c();
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(e) => {
                log::warn!("Failed to install SIGTERM handler: {e}");
                if let Err(e) = interrupt.await {
                    log::warn!("Failed to listen for Ctrl-C: {e}");
                }
                return;
            }
        };

        tokio::select! {
            result = interrupt => {
                if let Err(e) = result {
                    log::warn!("Failed to listen for Ctrl-C: {e}");
                } else {
                    log::info!("Received Ctrl-C");
                }
            }
            _ = terminate.recv() => {
                log::info!("Received SIGTERM");
            }
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            log::warn!("Failed to listen for Ctrl-C: {e}");
        } else {
            log::info!("Received Ctrl-C");
        }
    }
}

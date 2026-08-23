use anyhow::Result;
use config::StreamerArgs;
use std::io::Write;
use std::time::Duration;

mod engine;
pub mod queue;
mod runtime;
mod source;

pub use engine::{Aes67Sender, SenderConfig};
pub use source::{SendAudioSource, SilenceSource};

/// Runs one file-backed AES67 send session.
pub async fn send_file(args: StreamerArgs) -> Result<()> {
    let sender_config = SenderConfig {
        target_sample_rate: 48_000,
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

    let mut sender = Aes67Sender::new(
        &args.file,
        &args.address,
        args.port,
        args.interface.as_deref(),
        sender_config,
    )
    .await?;

    print_sdp(&sender.get_sdp())?;

    if let Some(path) = args.sdp_output.as_deref() {
        sender.write_sdp_file(path)?;
        log::info!("Wrote SDP file: {path}");
    }

    runtime::RuntimeSupervisor::new()
        .run_sender(&mut sender)
        .await
}

fn print_sdp(sdp: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(sdp.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

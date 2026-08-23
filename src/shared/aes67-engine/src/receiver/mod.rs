pub mod output;
mod runtime;
mod session;

use anyhow::{Context, Result};
use config::PlayerArgs;
use std::io::Write;
use std::time::Duration;

pub use output::list_output_devices;
pub use session::{Aes67Receiver, ReceiverConfig, ReceiverStats};

/// Receives one AES67 stream and plays it through the selected local output.
pub async fn listen(args: PlayerArgs) -> Result<()> {
    let receiver_config = ReceiverConfig {
        output_device: args.output_device.clone(),
        latency_ms: args.latency_ms,
        duration: args.duration_seconds.map(Duration::from_secs_f64),
        verbose: args.verbose,
        test_null_output: args.test_null_output,
    };

    let mut receiver = Aes67Receiver::new(&args, receiver_config)
        .await
        .context("Failed to create AES67 receiver")?;
    print_sdp(&receiver.get_receiver_sdp())?;
    runtime::RuntimeSupervisor::new()
        .run_receiver(&mut receiver)
        .await
}

fn print_sdp(sdp: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(sdp.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

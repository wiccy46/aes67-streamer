use std::process;
use std::time::Duration;

mod output;
mod player;
mod runtime;

use output::list_output_devices;
use player::{Aes67Player, PlayerConfig};
use runtime::RuntimeSupervisor;

#[tokio::main]
async fn main() {
    let args = match config::parse_player_args() {
        Ok(args) => args,
        Err(e) => {
            if config::is_display_control_error(&e) {
                print!("{e}");
                process::exit(0);
            }
            eprintln!("Error parsing arguments: {e}");
            process::exit(1);
        }
    };

    let default_log_level = if args.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_log_level))
        .init();

    if args.list_devices {
        match list_output_devices() {
            Ok(devices) => {
                print!("{devices}");
                process::exit(0);
            }
            Err(e) => {
                log::error!("Failed to list audio devices: {e:#}");
                process::exit(1);
            }
        }
    }

    let player_config = PlayerConfig {
        output_device: args.output_device.clone(),
        latency_ms: args.latency_ms,
        duration: args.duration_seconds.map(Duration::from_secs_f64),
        verbose: args.verbose,
        test_null_output: args.test_null_output,
    };

    let mut player = match Aes67Player::new(&args, player_config).await {
        Ok(player) => player,
        Err(e) => {
            log::error!("Failed to create AES67 player: {e:#}");
            process::exit(1);
        }
    };

    let supervisor = RuntimeSupervisor::new();
    if let Err(e) = supervisor.run_player(&mut player).await {
        log::error!("AES67 playback failed: {e:#}");
        process::exit(1);
    }

    log::info!("AES67 player completed successfully");
}

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::process::{Command as StdCommand, Output, Stdio};
use std::time::Duration;
use tokio::time;

#[tokio::test]
async fn streamer_to_player_null_output_decodes_loopback_rtp() -> Result<()> {
    let player_binary = player_binary_path();
    let streamer_binary = streamer_binary_path()?;
    let audio_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aes67-streamer/tests/resources/audio-formats/tone.wav")
        .canonicalize()
        .context("failed to locate loopback test audio file")?;
    let address = "239.1.4.1";
    let port = "55200";

    let player = tokio::process::Command::new(player_binary)
        .kill_on_drop(true)
        .arg("--address")
        .arg(address)
        .arg("--port")
        .arg(port)
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--output")
        .arg("null")
        .arg("--latency-ms")
        .arg("10")
        .arg("--duration-seconds")
        .arg("4")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start aes67-player")?;

    time::sleep(Duration::from_millis(100)).await;

    let mut streamer = tokio::process::Command::new(streamer_binary)
        .kill_on_drop(true)
        .arg("--file")
        .arg(audio_file)
        .arg("--address")
        .arg(address)
        .arg("--port")
        .arg(port)
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--duration-seconds")
        .arg("1")
        .spawn()
        .context("failed to start aes67-streamer")?;

    let streamer_status = time::timeout(Duration::from_secs(5), streamer.wait())
        .await
        .context("timed out waiting for streamer")??;
    assert!(
        streamer_status.success(),
        "aes67-streamer exited with {streamer_status}"
    );

    let player_output = time::timeout(Duration::from_secs(5), player.wait_with_output())
        .await
        .context("timed out waiting for player")??;
    let player_logs = process_output_text(&player_output);
    assert!(
        player_output.status.success(),
        "aes67-player exited with {}\n{}",
        player_output.status,
        player_logs
    );
    assert_eq!(summary_value(&player_logs, "Packets received")?, 100);
    assert_eq!(summary_value(&player_logs, "Packets accepted")?, 100);
    assert_eq!(summary_value(&player_logs, "Packets decoded")?, 100);
    assert_eq!(summary_value(&player_logs, "Frames decoded")?, 4800);
    assert_eq!(summary_value(&player_logs, "RTP silence frames")?, 0);
    assert_eq!(summary_value(&player_logs, "Jitter lost packets")?, 0);
    assert_eq!(summary_value(&player_logs, "Jitter late packets")?, 0);
    assert_eq!(summary_value(&player_logs, "Jitter duplicate packets")?, 0);
    assert_eq!(
        summary_value(&player_logs, "Jitter dropped-full packets")?,
        0
    );
    assert_eq!(
        summary_value(&player_logs, "Jitter timestamp discontinuities")?,
        0
    );
    assert_eq!(summary_value(&player_logs, "Output frames")?, 4800);
    assert_eq!(summary_value(&player_logs, "Output samples")?, 9600);
    assert_eq!(summary_value(&player_logs, "Output silence frames")?, 0);
    assert_eq!(summary_value(&player_logs, "Output dropped samples")?, 0);

    Ok(())
}

fn process_output_text(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn summary_value(logs: &str, label: &str) -> Result<u64> {
    let label_prefix = format!("{label}:");
    let line = logs
        .lines()
        .find(|line| {
            line.rsplit_once(']')
                .map(|(_, message)| message.trim_start().starts_with(&label_prefix))
                .unwrap_or(false)
        })
        .with_context(|| format!("player summary missing '{label}' in logs:\n{logs}"))?;
    let value = line
        .rsplit_once(':')
        .with_context(|| format!("player summary line has no ':' separator: {line}"))?
        .1
        .trim()
        .parse::<u64>()
        .with_context(|| format!("player summary value is not an integer: {line}"))?;

    Ok(value)
}

fn player_binary_path() -> PathBuf {
    option_env!("CARGO_BIN_EXE_aes67-player")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/aes67-player")
        })
}

fn streamer_binary_path() -> Result<PathBuf> {
    if let Some(path) = option_env!("CARGO_BIN_EXE_aes67-streamer") {
        return Ok(PathBuf::from(path));
    }

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .context("failed to locate workspace root")?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = StdCommand::new(cargo)
        .arg("build")
        .arg("-p")
        .arg("aes67-streamer")
        .current_dir(&workspace_root)
        .status()
        .context("failed to build aes67-streamer for player E2E")?;

    if !status.success() {
        return Err(anyhow!(
            "failed to build aes67-streamer for player E2E: {status}"
        ));
    }

    Ok(workspace_root.join("target/debug/aes67-streamer"))
}

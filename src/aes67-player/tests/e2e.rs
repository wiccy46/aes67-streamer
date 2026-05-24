use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time;

#[tokio::test]
async fn basic_args_decode_loopback_rtp() -> Result<()> {
    run_loopback_receive_test(ReceiveMode::BasicArgs).await
}

#[tokio::test]
async fn sdp_file_decodes_loopback_rtp() -> Result<()> {
    run_loopback_receive_test(ReceiveMode::SdpFile).await
}

#[tokio::test]
async fn sdp_file_controls_payload_type_and_packet_time() -> Result<()> {
    run_loopback_receive_test(ReceiveMode::SdpMetadata).await
}

#[tokio::test]
async fn sender_filter_accepts_matching_streamer_address() -> Result<()> {
    run_loopback_receive_test(ReceiveMode::SenderFilter).await
}

#[tokio::test]
async fn sender_filter_rejects_non_matching_streamer_address() -> Result<()> {
    let player_binary = player_binary_path();
    let streamer_binary = streamer_binary_path()?;
    let audio_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aes67-streamer/tests/resources/audio-formats/tone.wav")
        .canonicalize()
        .context("failed to locate loopback test audio file")?;
    let (address, port) = loopback_multicast_endpoint();

    let player = tokio::process::Command::new(player_binary)
        .kill_on_drop(true)
        .arg("--address")
        .arg(&address)
        .arg("--port")
        .arg(&port)
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--sender")
        .arg("127.0.0.2")
        .arg("--test-null-output")
        .arg("--latency-ms")
        .arg("10")
        .arg("--duration-seconds")
        .arg("1")
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
        .arg(&address)
        .arg("--port")
        .arg(&port)
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
        !player_output.status.success(),
        "aes67-player should reject all packets from a non-matching sender\n{player_logs}"
    );
    assert!(
        player_logs.contains("no RTP audio packets were decoded"),
        "player should report that no packets were decoded\n{player_logs}"
    );

    Ok(())
}

async fn run_loopback_receive_test(receive_mode: ReceiveMode) -> Result<()> {
    let player_binary = player_binary_path();
    let streamer_binary = streamer_binary_path()?;
    let audio_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aes67-streamer/tests/resources/audio-formats/tone.wav")
        .canonicalize()
        .context("failed to locate loopback test audio file")?;
    let (address, port) = loopback_multicast_endpoint();
    let session = receive_mode.session();
    let sdp_file = match receive_mode {
        ReceiveMode::BasicArgs | ReceiveMode::SenderFilter => None,
        ReceiveMode::SdpFile | ReceiveMode::SdpMetadata => {
            Some(write_loopback_sdp(&address, &port, session)?)
        }
    };
    let streamer_config = match receive_mode {
        ReceiveMode::SdpMetadata => Some(write_loopback_streamer_config(
            &audio_file,
            &address,
            &port,
            session,
        )?),
        ReceiveMode::BasicArgs | ReceiveMode::SdpFile | ReceiveMode::SenderFilter => None,
    };

    let mut player_command = tokio::process::Command::new(player_binary);
    player_command
        .kill_on_drop(true)
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--test-null-output")
        .arg("--latency-ms")
        .arg("10")
        .arg("--duration-seconds")
        .arg("4")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if receive_mode.uses_sender_filter() {
        player_command.arg("--sender").arg("127.0.0.1");
    }

    match sdp_file.as_ref() {
        Some(sdp) => {
            player_command.arg("--sdp").arg(sdp.path());
        }
        None => {
            player_command
                .arg("--address")
                .arg(&address)
                .arg("--port")
                .arg(&port);
        }
    }

    let player = player_command
        .spawn()
        .context("failed to start aes67-player")?;

    time::sleep(Duration::from_millis(100)).await;

    let mut streamer_command = tokio::process::Command::new(streamer_binary);
    streamer_command.kill_on_drop(true);

    match streamer_config.as_ref() {
        Some(config) => {
            streamer_command.arg("--config").arg(config.path());
        }
        None => {
            streamer_command
                .arg("--file")
                .arg(&audio_file)
                .arg("--address")
                .arg(&address)
                .arg("--port")
                .arg(&port)
                .arg("--interface")
                .arg("127.0.0.1")
                .arg("--duration-seconds")
                .arg("1");
        }
    }

    let mut streamer = streamer_command
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
    assert_eq!(
        summary_value(&player_logs, "Packets received")?,
        session.expected_packets
    );
    assert_eq!(
        summary_value(&player_logs, "Packets accepted")?,
        session.expected_packets
    );
    assert_eq!(
        summary_value(&player_logs, "Packets decoded")?,
        session.expected_packets
    );
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

#[derive(Debug, Clone, Copy)]
enum ReceiveMode {
    BasicArgs,
    SdpFile,
    SdpMetadata,
    SenderFilter,
}

impl ReceiveMode {
    fn session(self) -> LoopbackSession {
        match self {
            Self::BasicArgs | Self::SdpFile | Self::SenderFilter => LoopbackSession {
                payload_type: 97,
                packet_time_ms: 1,
                expected_packets: 100,
            },
            Self::SdpMetadata => LoopbackSession {
                payload_type: 101,
                packet_time_ms: 2,
                expected_packets: 50,
            },
        }
    }

    fn uses_sender_filter(self) -> bool {
        matches!(self, Self::SenderFilter)
    }
}

#[derive(Debug, Clone, Copy)]
struct LoopbackSession {
    payload_type: u8,
    packet_time_ms: u32,
    expected_packets: u64,
}

fn process_output_text(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn loopback_multicast_endpoint() -> (String, String) {
    let time_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let seed = time_seed ^ std::process::id();
    let group_octet_3 = 1 + (seed % 200);
    let group_octet_4 = 1 + ((seed / 200) % 200);
    let port = 55200 + (seed % 1000) as u16;

    (
        format!("239.1.{group_octet_3}.{group_octet_4}"),
        port.to_string(),
    )
}

struct TempSdpFile {
    path: PathBuf,
}

impl TempSdpFile {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempSdpFile {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

struct TempConfigFile {
    path: PathBuf,
}

impl TempConfigFile {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempConfigFile {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

fn write_loopback_sdp(address: &str, port: &str, session: LoopbackSession) -> Result<TempSdpFile> {
    let path = std::env::temp_dir().join(format!(
        "aes67-player-loopback-{}-{port}.sdp",
        std::process::id()
    ));
    let contents = format!(
        concat!(
            "v=0\r\n",
            "o=- 123456 123456 IN IP4 127.0.0.1\r\n",
            "s=AES67 Player E2E\r\n",
            "c=IN IP4 {address}/32\r\n",
            "t=0 0\r\n",
            "m=audio {port} RTP/AVP {payload_type}\r\n",
            "a=rtpmap:{payload_type} L24/48000/2\r\n",
            "a=ptime:{packet_time_ms}\r\n",
            "a=recvonly\r\n",
        ),
        address = address,
        port = port,
        payload_type = session.payload_type,
        packet_time_ms = session.packet_time_ms,
    );
    fs::write(&path, contents)
        .with_context(|| format!("failed to write loopback SDP {}", path.display()))?;
    Ok(TempSdpFile { path })
}

fn write_loopback_streamer_config(
    audio_file: &Path,
    address: &str,
    port: &str,
    session: LoopbackSession,
) -> Result<TempConfigFile> {
    let path = std::env::temp_dir().join(format!(
        "aes67-player-streamer-{}-{port}.toml",
        std::process::id()
    ));
    let contents = format!(
        concat!(
            "[audio]\n",
            "file = {audio_file:?}\n",
            "\n",
            "[stream]\n",
            "address = {address:?}\n",
            "port = {port}\n",
            "interface = \"127.0.0.1\"\n",
            "payload_type = {payload_type}\n",
            "packet_time_ms = {packet_time_ms}\n",
            "sap = false\n",
        ),
        audio_file = audio_file.display().to_string(),
        address = address,
        port = port,
        payload_type = session.payload_type,
        packet_time_ms = session.packet_time_ms,
    );
    fs::write(&path, contents).with_context(|| {
        format!(
            "failed to write loopback streamer config {}",
            path.display()
        )
    })?;
    Ok(TempConfigFile { path })
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

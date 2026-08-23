use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::time;

static NEXT_LOOPBACK_SESSION: AtomicU32 = AtomicU32::new(0);

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
async fn sender_filter_accepts_matching_sender_address() -> Result<()> {
    run_loopback_receive_test(ReceiveMode::SenderFilter).await
}

#[cfg(unix)]
#[tokio::test]
async fn receiver_stops_gracefully_on_sigterm() -> Result<()> {
    run_shutdown_signal_test("-TERM", "Received SIGTERM").await
}

#[cfg(unix)]
#[tokio::test]
async fn receiver_stops_gracefully_on_sigint() -> Result<()> {
    run_shutdown_signal_test("-INT", "Received Ctrl-C").await
}

#[tokio::test]
async fn sender_filter_rejects_non_matching_sender_address() -> Result<()> {
    let receiver_binary = aes67_binary_path();
    let sender_binary = aes67_binary_path();
    let audio_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/resources/send/streaming/audio-formats/tone.wav")
        .canonicalize()
        .context("failed to locate loopback test audio file")?;
    let (address, port) = loopback_multicast_endpoint();

    let receiver = tokio::process::Command::new(receiver_binary)
        .kill_on_drop(true)
        .arg("receive")
        .arg("listen")
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
        .context("failed to start AES67 receiver")?;

    time::sleep(Duration::from_millis(100)).await;

    let mut sender = tokio::process::Command::new(sender_binary)
        .kill_on_drop(true)
        .arg("send")
        .arg("file")
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
        .context("failed to start AES67 sender")?;

    let sender_status = time::timeout(Duration::from_secs(5), sender.wait())
        .await
        .context("timed out waiting for sender")??;
    assert!(
        sender_status.success(),
        "AES67 sender exited with {sender_status}"
    );

    let receiver_output = time::timeout(Duration::from_secs(5), receiver.wait_with_output())
        .await
        .context("timed out waiting for receiver")??;
    let receiver_logs = process_output_text(&receiver_output);

    assert!(
        !receiver_output.status.success(),
        "receiver should reject all packets from a non-matching sender\n{receiver_logs}"
    );
    assert!(
        receiver_logs.contains("no RTP audio packets were decoded"),
        "receiver should report that no packets were decoded\n{receiver_logs}"
    );

    Ok(())
}

#[cfg(unix)]
async fn run_shutdown_signal_test(signal: &str, expected_signal_log: &str) -> Result<()> {
    let receiver_binary = aes67_binary_path();
    let sender_binary = aes67_binary_path();
    let audio_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/resources/send/streaming/audio-formats/tone.wav")
        .canonicalize()
        .context("failed to locate loopback test audio file")?;
    let (address, port) = loopback_multicast_endpoint();

    let receiver = tokio::process::Command::new(receiver_binary)
        .kill_on_drop(true)
        .arg("receive")
        .arg("listen")
        .arg("--address")
        .arg(&address)
        .arg("--port")
        .arg(&port)
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--test-null-output")
        .arg("--latency-ms")
        .arg("10")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start AES67 receiver")?;

    time::sleep(Duration::from_millis(100)).await;

    let mut sender = tokio::process::Command::new(sender_binary)
        .kill_on_drop(true)
        .arg("send")
        .arg("file")
        .arg("--file")
        .arg(audio_file)
        .arg("--address")
        .arg(&address)
        .arg("--port")
        .arg(&port)
        .arg("--interface")
        .arg("127.0.0.1")
        .spawn()
        .context("failed to start AES67 sender")?;

    let sender_status = time::timeout(Duration::from_secs(5), sender.wait())
        .await
        .context("timed out waiting for sender")??;
    assert!(
        sender_status.success(),
        "AES67 sender exited with {sender_status}"
    );

    let receiver_id = receiver
        .id()
        .context("receiver process should have an id")?;
    let signal_status = tokio::process::Command::new("kill")
        .arg(signal)
        .arg(receiver_id.to_string())
        .status()
        .await
        .with_context(|| format!("failed to send {signal} to receiver"))?;
    assert!(
        signal_status.success(),
        "kill {signal} failed with {signal_status}"
    );

    let receiver_output = time::timeout(Duration::from_secs(5), receiver.wait_with_output())
        .await
        .context("timed out waiting for receiver")??;
    let receiver_logs = process_output_text(&receiver_output);

    assert!(
        receiver_output.status.success(),
        "AES67 receiver exited with {}\n{}",
        receiver_output.status,
        receiver_logs
    );
    assert!(
        receiver_logs.contains(expected_signal_log),
        "receiver should log {expected_signal_log:?}\n{receiver_logs}"
    );
    assert!(
        receiver_logs.contains("Stop reason: shutdown requested"),
        "receiver should summarize shutdown reason\n{receiver_logs}"
    );
    assert!(
        summary_value(&receiver_logs, "Packets decoded")? > 0,
        "receiver should decode RTP before shutdown\n{receiver_logs}"
    );
    assert_eq!(summary_value(&receiver_logs, "RTP silence frames")?, 0);
    assert_eq!(summary_value(&receiver_logs, "Jitter lost packets")?, 0);
    assert_eq!(summary_value(&receiver_logs, "Output dropped samples")?, 0);

    Ok(())
}

async fn run_loopback_receive_test(receive_mode: ReceiveMode) -> Result<()> {
    let receiver_binary = aes67_binary_path();
    let sender_binary = aes67_binary_path();
    let audio_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/resources/send/streaming/audio-formats/tone.wav")
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
    let sender_config = match receive_mode {
        ReceiveMode::SdpMetadata => Some(write_loopback_sender_config(
            &audio_file,
            &address,
            &port,
            session,
        )?),
        ReceiveMode::BasicArgs | ReceiveMode::SdpFile | ReceiveMode::SenderFilter => None,
    };

    let mut receiver_command = tokio::process::Command::new(receiver_binary);
    receiver_command
        .kill_on_drop(true)
        .arg("receive")
        .arg("listen")
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
        receiver_command.arg("--sender").arg("127.0.0.1");
    }

    match sdp_file.as_ref() {
        Some(sdp) => {
            receiver_command.arg("--sdp").arg(sdp.path());
        }
        None => {
            receiver_command
                .arg("--address")
                .arg(&address)
                .arg("--port")
                .arg(&port);
        }
    }

    let receiver = receiver_command
        .spawn()
        .context("failed to start AES67 receiver")?;

    time::sleep(Duration::from_millis(100)).await;

    let mut sender_command = tokio::process::Command::new(sender_binary);
    sender_command.kill_on_drop(true).arg("send").arg("file");

    match sender_config.as_ref() {
        Some(config) => {
            sender_command.arg("--config").arg(config.path());
        }
        None => {
            sender_command
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

    let mut sender = sender_command
        .spawn()
        .context("failed to start AES67 sender")?;

    let sender_status = time::timeout(Duration::from_secs(5), sender.wait())
        .await
        .context("timed out waiting for sender")??;
    assert!(
        sender_status.success(),
        "AES67 sender exited with {sender_status}"
    );

    let receiver_output = time::timeout(Duration::from_secs(5), receiver.wait_with_output())
        .await
        .context("timed out waiting for receiver")??;
    let receiver_logs = process_output_text(&receiver_output);
    assert!(
        receiver_output.status.success(),
        "AES67 receiver exited with {}\n{}",
        receiver_output.status,
        receiver_logs
    );
    assert_eq!(
        summary_value(&receiver_logs, "Packets received")?,
        session.expected_packets
    );
    assert_eq!(
        summary_value(&receiver_logs, "Packets accepted")?,
        session.expected_packets
    );
    assert_eq!(
        summary_value(&receiver_logs, "Packets decoded")?,
        session.expected_packets
    );
    assert_eq!(summary_value(&receiver_logs, "Frames decoded")?, 4800);
    assert_eq!(summary_value(&receiver_logs, "RTP silence frames")?, 0);
    assert_eq!(summary_value(&receiver_logs, "Jitter lost packets")?, 0);
    assert_eq!(summary_value(&receiver_logs, "Jitter late packets")?, 0);
    assert_eq!(
        summary_value(&receiver_logs, "Jitter duplicate packets")?,
        0
    );
    assert_eq!(
        summary_value(&receiver_logs, "Jitter dropped-full packets")?,
        0
    );
    assert_eq!(
        summary_value(&receiver_logs, "Jitter timestamp discontinuities")?,
        0
    );
    assert_eq!(summary_value(&receiver_logs, "Output frames")?, 4800);
    assert_eq!(summary_value(&receiver_logs, "Output samples")?, 9600);
    assert_eq!(summary_value(&receiver_logs, "Output silence frames")?, 0);
    assert_eq!(summary_value(&receiver_logs, "Output dropped samples")?, 0);

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
    let session = NEXT_LOOPBACK_SESSION.fetch_add(1, Ordering::Relaxed);
    let group_octet_3 = 1 + (session % 200);
    let group_octet_4 = 1 + ((session / 200) % 200);
    let port = 55200 + (session % 1000) as u16;

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
        "aes67-receive-loopback-{}-{port}.sdp",
        std::process::id()
    ));
    let contents = format!(
        concat!(
            "v=0\r\n",
            "o=- 123456 123456 IN IP4 127.0.0.1\r\n",
            "s=AES67 Receive E2E\r\n",
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

fn write_loopback_sender_config(
    audio_file: &Path,
    address: &str,
    port: &str,
    session: LoopbackSession,
) -> Result<TempConfigFile> {
    let path = std::env::temp_dir().join(format!(
        "aes67-receive-send-{}-{port}.toml",
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
    fs::write(&path, contents)
        .with_context(|| format!("failed to write loopback sender config {}", path.display()))?;
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
        .with_context(|| format!("receiver summary missing '{label}' in logs:\n{logs}"))?;
    let value = line
        .rsplit_once(':')
        .with_context(|| format!("receiver summary line has no ':' separator: {line}"))?
        .1
        .trim()
        .parse::<u64>()
        .with_context(|| format!("receiver summary value is not an integer: {line}"))?;

    Ok(value)
}

fn aes67_binary_path() -> PathBuf {
    option_env!("CARGO_BIN_EXE_aes67")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/aes67")
        })
}

use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn once_mode_prints_real_sap_datagram_and_writes_sdp() {
    let port = free_udp_port();
    let temp = TempDir::new("once-sap");
    let sdp_dir = temp.path().join("sdp");
    let mut child = Command::new(env!("CARGO_BIN_EXE_aes67"))
        .arg("receive")
        .arg("discover")
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--once")
        .arg("--sdp-output-dir")
        .arg(&sdp_dir)
        .arg("--test-address")
        .arg("127.0.0.1")
        .arg("--test-port")
        .arg(port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("aes67 receive discover should start");

    let sender =
        UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("sender should bind");
    let packet = sap_packet(
        "v=0\r\n\
         s=CLI SAP Stream\r\n\
         c=IN IP4 239.69.83.9/32\r\n\
         m=audio 5020 RTP/AVP 103\r\n\
         a=rtpmap:103 L24/48000/2\r\n\
         a=ptime:1\r\n",
    );
    let target = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        sender
            .send_to(&packet, target)
            .expect("SAP datagram should send");
        if child
            .try_wait()
            .expect("discovery status should be readable")
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            let output = child
                .wait_with_output()
                .expect("discovery output should be readable after timeout");
            panic!(
                "aes67 receive discover did not exit after SAP datagram\n{}",
                output_text(&output)
            );
        }
        thread::sleep(Duration::from_millis(100));
    }

    let output = child
        .wait_with_output()
        .expect("discovery output should be readable");
    assert!(
        output.status.success(),
        "aes67 receive discover should exit successfully\n{}",
        output_text(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("+ CLI SAP Stream"), "stdout: {stdout}");
    assert!(stdout.contains("239.69.83.9:5020"), "stdout: {stdout}");
    assert!(stdout.contains("L24/48000/2"), "stdout: {stdout}");

    let sdp_file = sdp_dir.join("sap-127.0.0.1-1234.sdp");
    let sdp = fs::read_to_string(&sdp_file)
        .unwrap_or_else(|error| panic!("expected SDP file at {}: {error}", sdp_file.display()));
    assert!(sdp.contains("s=CLI SAP Stream"));
}

fn sap_packet(sdp: &str) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(0x20);
    packet.push(0x00);
    packet.extend_from_slice(&0x1234u16.to_be_bytes());
    packet.extend_from_slice(&[127, 0, 0, 1]);
    packet.extend_from_slice(b"application/sdp\0");
    packet.extend_from_slice(sdp.as_bytes());
    packet
}

fn free_udp_port() -> u16 {
    let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("free port probe should bind");
    socket
        .local_addr()
        .expect("free port local addr should be readable")
        .port()
}

fn output_text(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aes67-discover-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&path).expect("temp dir should be writable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos()
}

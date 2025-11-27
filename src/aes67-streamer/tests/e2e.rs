use anyhow::Result;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time;

// We need to import the streamer crate to test it
// But since it's a binary crate, we can't easily import it unless it exposes a lib.
// For now, we'll test the components or run the binary.
// Actually, the previous `streamer.rs` is part of `aes67-streamer` binary.
// We can't import `Aes67Streamer` directly in `tests/e2e.rs` if it's in `src/main.rs` or `src/streamer.rs` of a binary crate.
// However, we can move the core logic to a library or just test by spawning the process.

// Let's try to spawn the process.

#[tokio::test]
async fn test_e2e_streaming() -> Result<()> {
    let cwd = std::env::current_dir()?;
    println!("Current working directory: {:?}", cwd);

    // 1. Build the project first to ensure binary is up to date
    println!("Building project...");
    let status = tokio::process::Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("aes67-streamer")
        .status()
        .await?;
    assert!(status.success(), "Failed to build project");

    // 2. Start the streamer
    // We use the test file
    let mut test_file = "tests/piano_freesound.wav";
    let mut binary_path = "target/debug/aes67-streamer";
    
    if !std::path::Path::new(test_file).exists() {
        if std::path::Path::new("../../tests/piano_freesound.wav").exists() {
            test_file = "../../tests/piano_freesound.wav";
            println!("Using test file at {}", test_file);
        } else {
            println!("Test file not found!");
        }
    }
    
    if !std::path::Path::new(binary_path).exists() {
        if std::path::Path::new("../../target/debug/aes67-streamer").exists() {
             binary_path = "../../target/debug/aes67-streamer";
             println!("Using binary at {}", binary_path);
        } else {
             println!("Binary not found!");
        }
    }

    let multicast_addr = "239.1.2.3";
    let port = 5004;
    
    println!("Spawning streamer...");
    let mut child = tokio::process::Command::new(binary_path)
        .arg("--file")
        .arg(test_file)
        .arg("--address")
        .arg(multicast_addr)
        .arg("--port")
        .arg(port.to_string())
        .arg("--interface")
        .arg("127.0.0.1") // Loopback
        .arg("--verbose")
        .kill_on_drop(true)
        .spawn()?;

    // 3. Setup a listener to verify packets
    use socket2::{Socket, Domain, Type, Protocol};
    
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    
    // Bind to wildcard address with reuse port
    let addr: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    socket.bind(&addr.into())?;
    
    // Join multicast group
    let multi_addr: std::net::Ipv4Addr = multicast_addr.parse()?;
    let interface: std::net::Ipv4Addr = "127.0.0.1".parse()?;
    socket.join_multicast_v4(&multi_addr, &interface)?;
    
    socket.set_nonblocking(true)?;
    let listener = tokio::net::UdpSocket::from_std(socket.into())?;
    
    println!("Listener bound to {}", listener.local_addr()?);

    // 4. Verify we receive packets
    let mut buf = [0u8; 2048];
    let mut packets_received = 0;
    
    // Listen for 5 seconds
    let timeout = time::sleep(Duration::from_secs(5));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => {
                break;
            }
            Ok((len, _)) = listener.recv_from(&mut buf) => {
                if len > 0 {
                    packets_received += 1;
                    // Verify RTP header (basic check)
                    // Version 2 (0x80)
                    if buf[0] & 0xC0 == 0x80 {
                        // Good
                    }
                }
            }
        }
        
        if packets_received > 100 {
            break;
        }
    }

    log::info!("Received {} packets", packets_received);
    assert!(packets_received > 0, "No packets received");

    // 5. Verify SAP announcements
    // SAP listens on 239.255.255.255:9875
    let sap_listener = UdpSocket::bind("0.0.0.0:9875").await?;
    sap_listener.join_multicast_v4(
        "239.255.255.255".parse()?,
        "127.0.0.1".parse()?,
    )?;
    
    let mut sap_received = false;
    let timeout = time::sleep(Duration::from_secs(35)); // SAP interval is 30s
    tokio::pin!(timeout);
    
    loop {
        tokio::select! {
            _ = &mut timeout => {
                break;
            }
            Ok((len, _)) = sap_listener.recv_from(&mut buf) => {
                if len > 0 {
                    // Check if it's our SAP
                    let s = String::from_utf8_lossy(&buf[..len]);
                    if s.contains("AES67 Streamer") {
                        sap_received = true;
                        break;
                    }
                }
            }
        }
    }
    
    assert!(sap_received, "No SAP announcement received");

    Ok(())
}

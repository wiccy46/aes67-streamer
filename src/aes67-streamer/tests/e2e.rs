use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time;

#[tokio::test]
async fn test_e2e_streaming_with_cli_args() -> Result<()> {
    run_streaming_test(StreamerArgs::Cli).await
}

#[tokio::test]
async fn test_e2e_streaming_with_config_file() -> Result<()> {
    run_streaming_test(StreamerArgs::ConfigFile).await
}

#[cfg(unix)]
#[tokio::test]
async fn test_e2e_streaming_stops_gracefully_on_sigterm() -> Result<()> {
    run_streaming_test(StreamerArgs::Sigterm).await
}

#[cfg(unix)]
#[tokio::test]
async fn test_e2e_streaming_stops_gracefully_on_sigint() -> Result<()> {
    run_streaming_test(StreamerArgs::Sigint).await
}

enum StreamerArgs {
    Cli,
    ConfigFile,
    Sigterm,
    Sigint,
}

async fn run_streaming_test(args_source: StreamerArgs) -> Result<()> {
    let test_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/piano_freesound.wav")
        .canonicalize()?;
    let binary_path = option_env!("CARGO_BIN_EXE_aes67-streamer")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/aes67-streamer")
        });
    let (multicast_addr, port) = match args_source {
        StreamerArgs::Cli => ("239.1.2.3", 55005),
        StreamerArgs::ConfigFile => ("239.1.2.4", 55006),
        StreamerArgs::Sigterm => ("239.1.2.5", 55007),
        StreamerArgs::Sigint => ("239.1.2.6", 55008),
    };

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;
    socket.bind(&addr.into())?;

    let multi_addr: std::net::Ipv4Addr = multicast_addr.parse()?;
    let interface: std::net::Ipv4Addr = "127.0.0.1".parse()?;
    socket.join_multicast_v4(&multi_addr, &interface)?;
    socket.set_nonblocking(true)?;
    let listener = tokio::net::UdpSocket::from_std(socket.into())?;

    println!("Listener bound to {}", listener.local_addr()?);

    let mut command = tokio::process::Command::new(binary_path);
    command.kill_on_drop(true);

    match args_source {
        StreamerArgs::Cli => {}
        StreamerArgs::ConfigFile => {
            command.arg("--config").arg(resource_config_path());
        }
        StreamerArgs::Sigterm => {}
        StreamerArgs::Sigint => {}
    };

    if matches!(
        args_source,
        StreamerArgs::Cli | StreamerArgs::Sigterm | StreamerArgs::Sigint
    ) {
        command
            .arg("--file")
            .arg(&test_file)
            .arg("--address")
            .arg(multicast_addr)
            .arg("--port")
            .arg(port.to_string())
            .arg("--interface")
            .arg("127.0.0.1");
    }

    if !matches!(args_source, StreamerArgs::Sigterm | StreamerArgs::Sigint) {
        command.arg("--duration-seconds").arg("2");
    }

    let mut child = command.spawn()?;

    let mut buf = [0u8; 2048];
    let mut packets_received = 0;

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
                    assert!(len >= 12, "RTP packet should include a header");
                    assert_eq!(buf[0] >> 6, 2, "RTP version should be 2");
                    assert_eq!(buf[1] & 0x7f, 97, "Payload type should be dynamic AES67 L24");
                }
            }
        }

        if packets_received > 100 {
            break;
        }
    }

    assert!(packets_received > 0, "No packets received");
    println!("Received {packets_received} RTP packets");

    if matches!(args_source, StreamerArgs::Sigterm | StreamerArgs::Sigint) {
        let child_id = child.id().expect("child process should have an id");
        let signal = match args_source {
            StreamerArgs::Sigterm => "-TERM",
            StreamerArgs::Sigint => "-INT",
            _ => unreachable!("non-signal streamer args should not reach signal branch"),
        };
        let status = tokio::process::Command::new("kill")
            .arg(signal)
            .arg(child_id.to_string())
            .status()
            .await?;
        assert!(status.success(), "failed to send {signal} to streamer");
    }

    let status = time::timeout(Duration::from_secs(5), child.wait()).await??;
    assert!(status.success(), "streamer exited with {status}");

    Ok(())
}

fn resource_config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/resources/e2e-streamer.toml")
}

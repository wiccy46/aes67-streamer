use anyhow::Result;
use hound::{SampleFormat, WavSpec, WavWriter};
use socket2::{Domain, Protocol, Socket, Type};
use std::io::ErrorKind;
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

#[tokio::test]
async fn test_e2e_loop_playback_streams_past_end_of_short_file() -> Result<()> {
    run_streaming_test(StreamerArgs::LoopPlayback).await
}

#[tokio::test]
async fn test_e2e_stream_metadata_controls_rtp_header() -> Result<()> {
    run_streaming_test(StreamerArgs::Metadata).await
}

#[derive(Clone, Copy)]
enum StreamerArgs {
    Cli,
    ConfigFile,
    Sigterm,
    Sigint,
    LoopPlayback,
    Metadata,
}

async fn run_streaming_test(args_source: StreamerArgs) -> Result<()> {
    let short_loop_file = if matches!(args_source, StreamerArgs::LoopPlayback) {
        Some(create_short_loop_wav()?)
    } else {
        None
    };
    let test_file = match short_loop_file.as_ref() {
        Some(path) => path.clone(),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/piano_freesound.wav")
            .canonicalize()?,
    };
    let binary_path = option_env!("CARGO_BIN_EXE_aes67-streamer")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/aes67-streamer")
        });
    let (multicast_addr, preferred_port) = match args_source {
        StreamerArgs::Cli => ("239.1.2.3", 55005),
        StreamerArgs::ConfigFile => ("239.1.2.4", 55006),
        StreamerArgs::Sigterm => ("239.1.2.5", 55007),
        StreamerArgs::Sigint => ("239.1.2.6", 55008),
        StreamerArgs::LoopPlayback => ("239.1.2.7", 55009),
        StreamerArgs::Metadata => ("239.1.2.8", 55010),
    };

    let (listener, port) = bind_rtp_listener(multicast_addr, preferred_port)?;

    println!("Listener bound to {}", listener.local_addr()?);

    let mut command = tokio::process::Command::new(binary_path);
    command.kill_on_drop(true);

    match args_source {
        StreamerArgs::Cli => {}
        StreamerArgs::ConfigFile => {
            command.arg("--config").arg(resource_config_path());
        }
        StreamerArgs::Metadata => {
            command.arg("--config").arg(resource_metadata_config_path());
        }
        StreamerArgs::Sigterm => {}
        StreamerArgs::Sigint => {}
        StreamerArgs::LoopPlayback => {}
    };

    if matches!(
        args_source,
        StreamerArgs::Cli
            | StreamerArgs::Sigterm
            | StreamerArgs::Sigint
            | StreamerArgs::LoopPlayback
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

    if matches!(args_source, StreamerArgs::LoopPlayback) {
        command.arg("--loop");
    }

    if !matches!(args_source, StreamerArgs::Sigterm | StreamerArgs::Sigint) {
        let duration = if matches!(args_source, StreamerArgs::LoopPlayback) {
            "0.2"
        } else {
            "2"
        };
        command.arg("--duration-seconds").arg(duration);
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
                    let expected_payload_type = if matches!(args_source, StreamerArgs::Metadata) {
                        101
                    } else {
                        97
                    };
                    assert_eq!(
                        buf[1] & 0x7f,
                        expected_payload_type,
                        "Payload type should match stream metadata"
                    );

                    if matches!(args_source, StreamerArgs::Metadata) {
                        assert_eq!(
                            u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
                            3735928559,
                            "SSRC should match stream metadata"
                        );
                    } else if matches!(args_source, StreamerArgs::Cli | StreamerArgs::ConfigFile) {
                        let ssrc = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
                        assert_ne!(ssrc, 0, "generated SSRC should be non-zero");
                        assert_ne!(ssrc, 0x12345678, "generated SSRC should not use the old fixed default");
                    }
                }
            }
        }

        let target_packets = if matches!(args_source, StreamerArgs::LoopPlayback) {
            50
        } else {
            100
        };

        if packets_received > target_packets {
            break;
        }
    }

    assert!(packets_received > 0, "No packets received");
    if matches!(args_source, StreamerArgs::LoopPlayback) {
        assert!(
            packets_received > 50,
            "loop playback should continue past the two-packet short file, got {packets_received}"
        );
    }
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
    if let Some(path) = short_loop_file {
        std::fs::remove_file(path).ok();
    }
    assert!(status.success(), "streamer exited with {status}");

    Ok(())
}

fn resource_config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/resources/e2e-streamer.toml")
}

fn resource_metadata_config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/resources/e2e-metadata.toml")
}

fn bind_rtp_listener(
    multicast_addr: &str,
    preferred_port: u16,
) -> Result<(tokio::net::UdpSocket, u16)> {
    match bind_rtp_listener_to_port(multicast_addr, preferred_port) {
        Ok(listener) => {
            let port = listener.local_addr()?.port();
            Ok((listener, port))
        }
        Err(error) if error.kind() == ErrorKind::AddrInUse => {
            let listener = bind_rtp_listener_to_port(multicast_addr, 0)?;
            let port = listener.local_addr()?.port();
            Ok((listener, port))
        }
        Err(error) => Err(error.into()),
    }
}

fn bind_rtp_listener_to_port(
    multicast_addr: &str,
    port: u16,
) -> std::io::Result<tokio::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse().map_err(|error| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid listener address: {error}"),
        )
    })?;
    socket.bind(&addr.into())?;

    let multi_addr: std::net::Ipv4Addr = multicast_addr.parse().map_err(|error| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid multicast address: {error}"),
        )
    })?;
    let interface: std::net::Ipv4Addr = "127.0.0.1".parse().expect("loopback IP should parse");
    socket.join_multicast_v4(&multi_addr, &interface)?;
    socket.set_nonblocking(true)?;

    tokio::net::UdpSocket::from_std(socket.into())
}

fn create_short_loop_wav() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "aes67-streamer-loop-e2e-{}.wav",
        std::process::id()
    ));

    let spec = WavSpec {
        channels: 2,
        sample_rate: 48000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(&path, spec)?;

    for frame in 0..96 {
        let sample = (2.0 * std::f32::consts::PI * 997.0 * frame as f32 / 48000.0).sin() * 0.5;
        writer.write_sample(sample)?;
        writer.write_sample(sample)?;
    }

    writer.finalize()?;
    Ok(path)
}

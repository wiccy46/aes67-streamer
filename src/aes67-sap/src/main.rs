use anyhow::{Context, Result};
use network::{
    resolve_interface_ip, SapBrowser, SapBrowserConfig, SapRegistry, SapRegistryEvent, SapStream,
};
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;
use tokio::time::{self, Duration};

const STREAM_EXPIRY: Duration = Duration::from_secs(90);
const EXPIRE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() {
    let args = match config::parse_sap_args() {
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

    if let Err(e) = run_browser(args).await {
        log::error!("AES67 SAP browser failed: {e:#}");
        process::exit(1);
    }
}

async fn run_browser(args: config::SapArgs) -> Result<()> {
    let interface = resolve_interface_ip(&args.interface)
        .with_context(|| format!("Failed to resolve SAP interface {}", args.interface))?;
    let listen_address = args
        .listen_address
        .parse::<Ipv4Addr>()
        .with_context(|| format!("Invalid SAP listen address {}", args.listen_address))?;
    let browser = SapBrowser::new(SapBrowserConfig {
        address: listen_address,
        port: args.port,
        interface,
        recv_buffer_size: 65_536,
    })?;
    let mut registry = SapRegistry::new(STREAM_EXPIRY);
    let output_dir = args.sdp_output_dir.as_deref().map(Path::new);
    let mut buffer = vec![0u8; 65_536];

    log::info!(
        "Browsing SAP on {}:{} via interface {}",
        listen_address,
        args.port,
        interface
    );

    if args.once {
        loop {
            match browser.recv_message(&mut buffer).await {
                Ok(received) => {
                    if let Some(event) =
                        registry.apply_message(received.message, received.source, Instant::now())
                    {
                        emit_registry_event(&event, output_dir, false)?;
                        return Ok(());
                    }
                }
                Err(e) => log::warn!("Ignoring SAP packet: {e:#}"),
            }
        }
    }

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut expire_interval = time::interval(EXPIRE_CHECK_INTERVAL);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                log::info!("Stopping SAP browser");
                return Ok(());
            }
            _ = expire_interval.tick() => {
                for event in registry.expire(Instant::now()) {
                    emit_registry_event(&event, output_dir, true)?;
                }
            }
            received = browser.recv_message(&mut buffer) => {
                match received {
                    Ok(received) => {
                        if let Some(event) = registry.apply_message(received.message, received.source, Instant::now()) {
                            emit_registry_event(&event, output_dir, true)?;
                        }
                    }
                    Err(e) => log::warn!("Ignoring SAP packet: {e:#}"),
                }
            }
        }
    }
}

fn emit_registry_event(
    event: &SapRegistryEvent,
    output_dir: Option<&Path>,
    show_sdp_details: bool,
) -> Result<()> {
    if let Some(output_dir) = output_dir {
        if matches!(
            event,
            SapRegistryEvent::Added(_) | SapRegistryEvent::Updated(_)
        ) {
            write_sdp_file(output_dir, event_stream(event))?;
        }
    }

    println!("{}", format_registry_event(event, show_sdp_details));
    io::stdout().flush().context("Failed to flush stdout")?;

    Ok(())
}

fn write_sdp_file(output_dir: &Path, stream: &SapStream) -> Result<()> {
    let Some(sdp) = stream.message.sdp.as_deref() else {
        return Ok(());
    };

    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create SAP SDP output directory {}",
            output_dir.display()
        )
    })?;
    let path = sdp_output_path(output_dir, stream);
    std::fs::write(&path, sdp)
        .with_context(|| format!("Failed to write discovered SDP file {}", path.display()))?;

    Ok(())
}

fn sdp_output_path(output_dir: &Path, stream: &SapStream) -> PathBuf {
    output_dir.join(format!(
        "sap-{}-{:04x}.sdp",
        stream.key.origin_source, stream.key.message_hash
    ))
}

fn format_registry_event(event: &SapRegistryEvent, show_sdp_details: bool) -> String {
    let marker = match event {
        SapRegistryEvent::Added(_) => "+",
        SapRegistryEvent::Updated(_) => "=",
        SapRegistryEvent::Removed(_) | SapRegistryEvent::Expired(_) => "-",
    };
    let stream = event_stream(event);

    let line = if let Some(session) = stream.message.session.as_ref() {
        let name = session
            .session_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("(unnamed)");
        format!(
            "{marker} {name} {}:{} L24/{}/{} ptime={}ms source={} origin={}",
            session.address,
            session.port,
            session.sample_rate,
            session.channels,
            session.packet_time_ms,
            stream.source,
            stream.key.origin_source
        )
    } else {
        format!(
            "{marker} SAP hash={:04x} source={} origin={}",
            stream.key.message_hash, stream.source, stream.key.origin_source
        )
    };

    if show_sdp_details
        && matches!(
            event,
            SapRegistryEvent::Added(_) | SapRegistryEvent::Updated(_)
        )
    {
        if let Some(sdp) = stream.message.sdp.as_deref() {
            return format!("{line}\n{}", format_sdp_block(sdp));
        }
    }

    line
}

fn format_sdp_block(sdp: &str) -> String {
    let mut block = String::from("  SDP:\n");
    for line in sdp
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
    {
        block.push_str("    ");
        block.push_str(line);
        block.push('\n');
    }
    block.pop();
    block
}

fn event_stream(event: &SapRegistryEvent) -> &SapStream {
    match event {
        SapRegistryEvent::Added(stream)
        | SapRegistryEvent::Updated(stream)
        | SapRegistryEvent::Removed(stream)
        | SapRegistryEvent::Expired(stream) => stream,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use network::{
        parse_sdp, SapMessage, SapMessageKey, SapMessageType, SapRegistryEvent, SapStream,
    };
    use std::net::{Ipv4Addr, SocketAddr};
    use std::path::Path;
    use std::time::Instant;

    #[test]
    fn formats_added_stream_like_browse_event_line() {
        let line =
            format_registry_event(&SapRegistryEvent::Added(test_stream("Studio Main")), false);

        assert!(line.starts_with("+ "));
        assert!(line.contains("Studio Main"));
        assert!(line.contains("239.69.83.1:5004"));
        assert!(line.contains("L24/48000/2"));
        assert!(line.contains("ptime=1ms"));
        assert!(line.contains("origin=192.168.1.50"));
        assert!(!line.contains("SDP:"));
    }

    #[test]
    fn formats_removed_stream_with_minus_event_marker() {
        let line =
            format_registry_event(&SapRegistryEvent::Removed(test_stream("Studio Main")), true);

        assert!(line.starts_with("- "));
        assert!(line.contains("Studio Main"));
        assert!(!line.contains("SDP:"));
    }

    #[test]
    fn formats_added_stream_with_readable_sdp_block_when_requested() {
        let output =
            format_registry_event(&SapRegistryEvent::Added(test_stream("Studio Main")), true);

        assert!(output.starts_with("+ Studio Main"));
        assert!(output.contains("\n  SDP:\n"));
        assert!(output.contains("    v=0\n"));
        assert!(output.contains("    s=Studio Main\n"));
        assert!(output.contains("    c=IN IP4 239.69.83.1/32\n"));
        assert!(output.contains("    a=rtpmap:97 L24/48000/2"));
    }

    #[test]
    fn sdp_output_path_uses_origin_and_message_hash() {
        let stream = test_stream("Studio Main");
        let path = sdp_output_path(Path::new("discovered"), &stream);

        assert_eq!(
            path,
            Path::new("discovered").join("sap-192.168.1.50-1234.sdp")
        );
    }

    fn test_stream(name: &str) -> SapStream {
        let sdp = format!(
            "v=0\r\n\
             s={name}\r\n\
             c=IN IP4 239.69.83.1/32\r\n\
             m=audio 5004 RTP/AVP 97\r\n\
             a=rtpmap:97 L24/48000/2\r\n\
             a=ptime:1\r\n"
        );
        let session = parse_sdp(&sdp).expect("test SDP should parse");
        let key = SapMessageKey {
            origin_source: Ipv4Addr::new(192, 168, 1, 50),
            message_hash: 0x1234,
        };

        SapStream {
            key,
            message: SapMessage {
                key,
                message_type: SapMessageType::Announcement,
                payload_type: Some("application/sdp".to_string()),
                sdp: Some(sdp),
                session: Some(session),
            },
            source: SocketAddr::from(([192, 168, 1, 50], 9875)),
            first_seen: Instant::now(),
            last_seen: Instant::now(),
        }
    }
}

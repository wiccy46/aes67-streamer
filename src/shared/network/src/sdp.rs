use anyhow::{anyhow, Context, Result};
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEncoding {
    L24,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aes67SessionDescription {
    pub session_name: Option<String>,
    pub address: Ipv4Addr,
    pub ttl: Option<u8>,
    pub port: u16,
    pub payload_type: u8,
    pub encoding: AudioEncoding,
    pub sample_rate: u32,
    pub channels: u16,
    pub packet_time_ms: u32,
    pub ts_refclk: Option<String>,
    pub mediaclk: Option<String>,
}

impl Aes67SessionDescription {
    pub fn get_frames_per_packet(&self) -> u32 {
        self.sample_rate * self.packet_time_ms / 1000
    }
}

pub fn parse_sdp_file(path: impl AsRef<Path>) -> Result<Aes67SessionDescription> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read SDP file {}", path.display()))?;
    parse_sdp(&contents).with_context(|| format!("failed to parse SDP file {}", path.display()))
}

pub fn parse_sdp(input: &str) -> Result<Aes67SessionDescription> {
    let mut session_name = None;
    let mut connection = None;
    let mut media = None;
    let mut rtpmap = None;
    let mut packet_time_ms = None;
    let mut ts_refclk = None;
    let mut mediaclk = None;

    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(value) = line.strip_prefix("s=") {
            session_name = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("c=") {
            connection = Some(parse_connection(value)?);
        } else if let Some(value) = line.strip_prefix("m=") {
            if let Some(parsed) = parse_media(value)? {
                media = Some(parsed);
            }
        } else if let Some(value) = line.strip_prefix("a=rtpmap:") {
            rtpmap = Some(parse_rtpmap(value)?);
        } else if let Some(value) = line.strip_prefix("a=ptime:") {
            packet_time_ms = Some(parse_packet_time_ms(value)?);
        } else if let Some(value) = line.strip_prefix("a=ts-refclk:") {
            ts_refclk = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("a=mediaclk:") {
            mediaclk = Some(value.to_string());
        }
    }

    let (address, ttl) = connection.ok_or_else(|| anyhow!("SDP is missing c= connection line"))?;
    let media = media.ok_or_else(|| anyhow!("SDP is missing m=audio media line"))?;
    let rtpmap = rtpmap.ok_or_else(|| anyhow!("SDP is missing a=rtpmap attribute"))?;

    if media.payload_type != rtpmap.payload_type {
        return Err(anyhow!(
            "SDP payload type mismatch: m=audio uses {}, rtpmap uses {}",
            media.payload_type,
            rtpmap.payload_type
        ));
    }

    if rtpmap.encoding != AudioEncoding::L24 {
        return Err(anyhow!("only L24 audio is supported"));
    }

    if rtpmap.sample_rate != 48_000 {
        return Err(anyhow!(
            "only 48000 Hz AES67 streams are supported, got {} Hz",
            rtpmap.sample_rate
        ));
    }

    if !(1..=8).contains(&rtpmap.channels) {
        return Err(anyhow!(
            "first release supports 1 to 8 channels, got {}",
            rtpmap.channels
        ));
    }

    Ok(Aes67SessionDescription {
        session_name,
        address,
        ttl,
        port: media.port,
        payload_type: media.payload_type,
        encoding: rtpmap.encoding,
        sample_rate: rtpmap.sample_rate,
        channels: rtpmap.channels,
        packet_time_ms: packet_time_ms.unwrap_or(1),
        ts_refclk,
        mediaclk,
    })
}

#[derive(Debug, Clone, Copy)]
struct MediaDescription {
    port: u16,
    payload_type: u8,
}

#[derive(Debug, Clone, Copy)]
struct RtpMap {
    payload_type: u8,
    encoding: AudioEncoding,
    sample_rate: u32,
    channels: u16,
}

fn parse_connection(value: &str) -> Result<(Ipv4Addr, Option<u8>)> {
    let tokens = value.split_whitespace().collect::<Vec<_>>();
    if tokens.len() != 3 || tokens[0] != "IN" || tokens[1] != "IP4" {
        return Err(anyhow!("unsupported SDP connection line: c={value}"));
    }

    let mut address_parts = tokens[2].split('/');
    let address = address_parts
        .next()
        .ok_or_else(|| anyhow!("SDP connection address is empty"))?
        .parse::<Ipv4Addr>()
        .with_context(|| format!("invalid SDP connection address {}", tokens[2]))?;
    let ttl = match address_parts.next() {
        Some(ttl) => Some(
            ttl.parse::<u8>()
                .with_context(|| format!("invalid SDP multicast TTL {ttl}"))?,
        ),
        None => None,
    };

    Ok((address, ttl))
}

fn parse_media(value: &str) -> Result<Option<MediaDescription>> {
    let tokens = value.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(anyhow!("empty SDP media line"));
    }
    if tokens[0] != "audio" {
        return Ok(None);
    }
    if tokens.len() < 4 {
        return Err(anyhow!("SDP audio media line is incomplete: m={value}"));
    }
    if tokens[2] != "RTP/AVP" {
        return Err(anyhow!(
            "unsupported SDP transport {}; expected RTP/AVP",
            tokens[2]
        ));
    }

    let port = tokens[1]
        .parse::<u16>()
        .with_context(|| format!("invalid SDP audio port {}", tokens[1]))?;
    let payload_type = parse_payload_type(tokens[3])?;

    Ok(Some(MediaDescription { port, payload_type }))
}

fn parse_rtpmap(value: &str) -> Result<RtpMap> {
    let (payload_type, format) = value
        .split_once(char::is_whitespace)
        .ok_or_else(|| anyhow!("SDP rtpmap is incomplete: a=rtpmap:{value}"))?;
    let payload_type = parse_payload_type(payload_type)?;
    let format_tokens = format.split('/').collect::<Vec<_>>();
    if format_tokens.len() != 3 {
        return Err(anyhow!("unsupported SDP rtpmap format: {format}"));
    }

    let encoding = match format_tokens[0].to_ascii_uppercase().as_str() {
        "L24" => AudioEncoding::L24,
        other => {
            return Err(anyhow!(
                "unsupported SDP audio encoding {other}; expected L24"
            ));
        }
    };
    let sample_rate = format_tokens[1]
        .parse::<u32>()
        .with_context(|| format!("invalid SDP rtpmap sample rate {}", format_tokens[1]))?;
    let channels = format_tokens[2]
        .parse::<u16>()
        .with_context(|| format!("invalid SDP rtpmap channel count {}", format_tokens[2]))?;

    Ok(RtpMap {
        payload_type,
        encoding,
        sample_rate,
        channels,
    })
}

fn parse_payload_type(value: &str) -> Result<u8> {
    let payload_type = value
        .parse::<u8>()
        .with_context(|| format!("invalid RTP payload type {value}"))?;
    if !(96..=127).contains(&payload_type) {
        return Err(anyhow!(
            "L24 RTP payload type must be dynamic, between 96 and 127"
        ));
    }

    Ok(payload_type)
}

fn parse_packet_time_ms(value: &str) -> Result<u32> {
    let packet_time = value
        .parse::<f64>()
        .with_context(|| format!("invalid SDP packet time {value}"))?;
    if !packet_time.is_finite() || packet_time <= 0.0 {
        return Err(anyhow!("SDP packet time must be greater than zero"));
    }
    if packet_time.fract() != 0.0 {
        return Err(anyhow!(
            "only whole-millisecond packet times are supported, got {packet_time}"
        ));
    }

    Ok(packet_time as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repository_example_sdp() {
        let session = parse_sdp(include_str!("../../../../tests/example.sdp")).unwrap();

        assert_eq!(session.session_name.as_deref(), Some("AES67 Streamer"));
        assert_eq!(session.address, Ipv4Addr::new(239, 192, 1, 1));
        assert_eq!(session.ttl, Some(32));
        assert_eq!(session.port, 5004);
        assert_eq!(session.payload_type, 97);
        assert_eq!(session.encoding, AudioEncoding::L24);
        assert_eq!(session.sample_rate, 48_000);
        assert_eq!(session.channels, 2);
        assert_eq!(session.packet_time_ms, 1);
        assert_eq!(session.get_frames_per_packet(), 48);
    }

    #[test]
    fn parses_crlf_sdp_with_clock_metadata() {
        let session = parse_sdp(
            "v=0\r\n\
             s=Clocked Stream\r\n\
             c=IN IP4 239.69.67.67/32\r\n\
             m=audio 5004 RTP/AVP 101\r\n\
             a=rtpmap:101 L24/48000/8\r\n\
             a=ptime:2\r\n\
             a=ts-refclk:ptp=IEEE1588-2008:00-11-22-ff-fe-33-44-55:0\r\n\
             a=mediaclk:direct=0\r\n",
        )
        .unwrap();

        assert_eq!(session.session_name.as_deref(), Some("Clocked Stream"));
        assert_eq!(session.address, Ipv4Addr::new(239, 69, 67, 67));
        assert_eq!(session.payload_type, 101);
        assert_eq!(session.channels, 8);
        assert_eq!(session.packet_time_ms, 2);
        assert_eq!(session.get_frames_per_packet(), 96);
        assert_eq!(
            session.ts_refclk.as_deref(),
            Some("ptp=IEEE1588-2008:00-11-22-ff-fe-33-44-55:0")
        );
        assert_eq!(session.mediaclk.as_deref(), Some("direct=0"));
    }

    #[test]
    fn defaults_packet_time_to_one_millisecond() {
        let session = parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
             m=audio 5004 RTP/AVP 97\n\
             a=rtpmap:97 L24/48000/2\n",
        )
        .unwrap();

        assert_eq!(session.packet_time_ms, 1);
    }

    #[test]
    fn rejects_missing_required_lines() {
        assert!(parse_sdp("m=audio 5004 RTP/AVP 97\na=rtpmap:97 L24/48000/2\n").is_err());
        assert!(parse_sdp("c=IN IP4 239.192.1.1/32\na=rtpmap:97 L24/48000/2\n").is_err());
        assert!(parse_sdp("c=IN IP4 239.192.1.1/32\nm=audio 5004 RTP/AVP 97\n").is_err());
    }

    #[test]
    fn rejects_payload_type_mismatch() {
        let result = parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
             m=audio 5004 RTP/AVP 97\n\
             a=rtpmap:98 L24/48000/2\n",
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_static_payload_type_for_l24() {
        let result = parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
             m=audio 5004 RTP/AVP 95\n\
             a=rtpmap:95 L24/48000/2\n",
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_unsupported_format_values() {
        assert!(parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
                 m=audio 5004 RTP/AVP 97\n\
                 a=rtpmap:97 L16/48000/2\n"
        )
        .is_err());
        assert!(parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
                 m=audio 5004 RTP/AVP 97\n\
                 a=rtpmap:97 L24/44100/2\n"
        )
        .is_err());
        assert!(parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
                 m=audio 5004 RTP/AVP 97\n\
                 a=rtpmap:97 L24/48000/9\n"
        )
        .is_err());
        assert!(parse_sdp(
            "c=IN IP4 239.192.1.1/32\n\
                 m=audio 5004 RTP/AVP 97\n\
                 a=rtpmap:97 L24/48000/2\n\
                 a=ptime:0.5\n"
        )
        .is_err());
    }
}

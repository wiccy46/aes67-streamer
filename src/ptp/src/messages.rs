use anyhow::{anyhow, Result};
use std::convert::TryInto;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageType {
    Sync = 0x0,
    DelayReq = 0x1,
    PdelayReq = 0x2,
    PdelayResp = 0x3,
    FollowUp = 0x8,
    DelayResp = 0x9,
    PdelayRespFollowUp = 0xA,
    Announce = 0xB,
    Signaling = 0xC,
    Management = 0xD,
    Unknown,
}

impl From<u8> for MessageType {
    fn from(byte: u8) -> Self {
        match byte & 0x0F {
            0x0 => MessageType::Sync,
            0x1 => MessageType::DelayReq,
            0x2 => MessageType::PdelayReq,
            0x3 => MessageType::PdelayResp,
            0x8 => MessageType::FollowUp,
            0x9 => MessageType::DelayResp,
            0xA => MessageType::PdelayRespFollowUp,
            0xB => MessageType::Announce,
            0xC => MessageType::Signaling,
            0xD => MessageType::Management,
            _ => MessageType::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PtpHeader {
    pub message_type: MessageType,
    pub version: u8,
    pub domain_number: u8,
    pub correction_field: i64,
    pub source_port_identity: [u8; 10],
    pub sequence_id: u16,
    pub control_field: u8,
    pub log_message_interval: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClockIdentity([u8; 8]);

impl ClockIdentity {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    pub fn from_mac_address(mac: [u8; 6]) -> Self {
        Self([mac[0], mac[1], mac[2], 0xff, 0xfe, mac[3], mac[4], mac[5]])
    }

    pub fn from_local_ipv4(ip: std::net::Ipv4Addr) -> Self {
        let octets = ip.octets();
        Self([
            0x02, 0x00, 0x00, 0xff, 0xfe, octets[1], octets[2], octets[3],
        ])
    }

    pub fn as_bytes(&self) -> [u8; 8] {
        self.0
    }
}

impl fmt::Display for ClockIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if index > 0 {
                write!(f, "-")?;
            }
            write!(f, "{byte:02X}")?;
        }

        Ok(())
    }
}

impl PtpHeader {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 34 {
            return Err(anyhow!("Packet too short for PTP header"));
        }

        let message_type = MessageType::from(bytes[0]);
        let version = bytes[1] & 0x0F;
        let domain_number = bytes[4];

        let correction_field = i64::from_be_bytes(bytes[8..16].try_into()?);

        let mut source_port_identity = [0u8; 10];
        source_port_identity.copy_from_slice(&bytes[20..30]);

        let sequence_id = u16::from_be_bytes(bytes[30..32].try_into()?);
        let control_field = bytes[32];
        let log_message_interval = bytes[33] as i8;

        Ok(Self {
            message_type,
            version,
            domain_number,
            correction_field,
            source_port_identity,
            sequence_id,
            control_field,
            log_message_interval,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub seconds: u64,
    pub nanoseconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceMessage {
    pub domain_number: u8,
    pub grandmaster_identity: ClockIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayReqMessage {
    pub domain_number: u8,
    pub source_port_identity: [u8; 10],
    pub sequence_id: u16,
    pub origin_timestamp: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayRespMessage {
    pub domain_number: u8,
    pub sequence_id: u16,
    pub receive_timestamp: Timestamp,
    pub requesting_port_identity: [u8; 10],
}

impl AnnounceMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let header = PtpHeader::from_bytes(bytes)?;
        if header.message_type != MessageType::Announce {
            return Err(anyhow!("PTP message is not an Announce message"));
        }

        if bytes.len() < 61 {
            return Err(anyhow!("Packet too short for PTP Announce message"));
        }

        let mut grandmaster_identity = [0u8; 8];
        grandmaster_identity.copy_from_slice(&bytes[53..61]);

        Ok(Self {
            domain_number: header.domain_number,
            grandmaster_identity: ClockIdentity::from_bytes(grandmaster_identity),
        })
    }
}

impl DelayReqMessage {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0u8; 44];
        bytes[0] = MessageType::DelayReq as u8;
        bytes[1] = 0x02;
        bytes[2..4].copy_from_slice(&(44u16).to_be_bytes());
        bytes[4] = self.domain_number;
        bytes[20..30].copy_from_slice(&self.source_port_identity);
        bytes[30..32].copy_from_slice(&self.sequence_id.to_be_bytes());
        bytes[32] = 1;
        bytes[33] = 0x7f;
        bytes[34..44].copy_from_slice(&self.origin_timestamp.to_bytes());
        bytes
    }
}

impl DelayRespMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let header = PtpHeader::from_bytes(bytes)?;
        if header.message_type != MessageType::DelayResp {
            return Err(anyhow!("PTP message is not a DelayResp message"));
        }

        if bytes.len() < 54 {
            return Err(anyhow!("Packet too short for PTP DelayResp message"));
        }

        let mut requesting_port_identity = [0u8; 10];
        requesting_port_identity.copy_from_slice(&bytes[44..54]);

        Ok(Self {
            domain_number: header.domain_number,
            sequence_id: header.sequence_id,
            receive_timestamp: Timestamp::from_bytes(&bytes[34..44])?,
            requesting_port_identity,
        })
    }
}

impl Timestamp {
    pub fn from_nanos(nanos: u64) -> Self {
        Self {
            seconds: nanos / 1_000_000_000,
            nanoseconds: (nanos % 1_000_000_000) as u32,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 10 {
            return Err(anyhow!("Buffer too short for Timestamp"));
        }

        let seconds_msb = u16::from_be_bytes(bytes[0..2].try_into()?);
        let seconds_lsb = u32::from_be_bytes(bytes[2..6].try_into()?);
        let seconds = ((seconds_msb as u64) << 32) | (seconds_lsb as u64);

        let nanoseconds = u32::from_be_bytes(bytes[6..10].try_into()?);

        Ok(Self {
            seconds,
            nanoseconds,
        })
    }

    pub fn as_nanos(&self) -> u128 {
        (self.seconds as u128 * 1_000_000_000) + self.nanoseconds as u128
    }

    pub fn to_bytes(&self) -> [u8; 10] {
        let mut bytes = [0u8; 10];
        let seconds_msb = (self.seconds >> 32) as u16;
        let seconds_lsb = self.seconds as u32;
        bytes[0..2].copy_from_slice(&seconds_msb.to_be_bytes());
        bytes[2..6].copy_from_slice(&seconds_lsb.to_be_bytes());
        bytes[6..10].copy_from_slice(&self.nanoseconds.to_be_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_identity_formats_as_ptp_sdp_token() {
        let identity = ClockIdentity::from_bytes([0xac, 0xde, 0x48, 0xff, 0xfe, 0x23, 0x45, 0x67]);

        assert_eq!(identity.to_string(), "AC-DE-48-FF-FE-23-45-67");
    }

    #[test]
    fn clock_identity_can_be_derived_from_eui48_mac() {
        let identity = ClockIdentity::from_mac_address([0xac, 0xde, 0x48, 0x23, 0x45, 0x67]);

        assert_eq!(identity.to_string(), "AC-DE-48-FF-FE-23-45-67");
    }

    #[test]
    fn announce_message_extracts_grandmaster_identity() {
        let mut bytes = vec![0u8; 64];
        bytes[0] = 0x0b;
        bytes[1] = 0x02;
        bytes[4] = 7;
        bytes[53..61].copy_from_slice(&[0x00, 0x1d, 0xc1, 0xff, 0xfe, 0x12, 0x34, 0x56]);

        let announce = AnnounceMessage::from_bytes(&bytes).expect("announce should parse");

        assert_eq!(announce.domain_number, 7);
        assert_eq!(
            announce.grandmaster_identity.to_string(),
            "00-1D-C1-FF-FE-12-34-56"
        );
    }

    #[test]
    fn delay_req_message_serializes_header_and_origin_timestamp() {
        let source_port_identity = [0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01, 0x00, 0x01];
        let delay_req = DelayReqMessage {
            domain_number: 7,
            source_port_identity,
            sequence_id: 42,
            origin_timestamp: Timestamp {
                seconds: 12,
                nanoseconds: 345,
            },
        };

        let bytes = delay_req.to_bytes();
        let header = PtpHeader::from_bytes(&bytes).expect("delay request header should parse");
        let timestamp =
            Timestamp::from_bytes(&bytes[34..44]).expect("origin timestamp should parse");

        assert_eq!(bytes.len(), 44);
        assert_eq!(header.message_type, MessageType::DelayReq);
        assert_eq!(header.version, 2);
        assert_eq!(header.domain_number, 7);
        assert_eq!(header.source_port_identity, source_port_identity);
        assert_eq!(header.sequence_id, 42);
        assert_eq!(header.control_field, 1);
        assert_eq!(timestamp.seconds, 12);
        assert_eq!(timestamp.nanoseconds, 345);
    }

    #[test]
    fn delay_resp_message_extracts_receive_timestamp_and_requesting_port_identity() {
        let requesting_port_identity = [0x02, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x00, 0x01, 0x00, 0x01];
        let mut bytes = vec![0u8; 54];
        bytes[0] = 0x09;
        bytes[1] = 0x02;
        bytes[4] = 7;
        bytes[30..32].copy_from_slice(&42u16.to_be_bytes());
        bytes[34..44].copy_from_slice(
            &Timestamp {
                seconds: 20,
                nanoseconds: 800,
            }
            .to_bytes(),
        );
        bytes[44..54].copy_from_slice(&requesting_port_identity);

        let delay_resp = DelayRespMessage::from_bytes(&bytes).expect("delay response should parse");

        assert_eq!(delay_resp.domain_number, 7);
        assert_eq!(delay_resp.sequence_id, 42);
        assert_eq!(delay_resp.receive_timestamp.seconds, 20);
        assert_eq!(delay_resp.receive_timestamp.nanoseconds, 800);
        assert_eq!(
            delay_resp.requesting_port_identity,
            requesting_port_identity
        );
    }
}

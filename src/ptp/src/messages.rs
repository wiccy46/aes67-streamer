use anyhow::{Result, anyhow};
use std::convert::TryInto;

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

#[derive(Debug, Clone, Copy)]
pub struct Timestamp {
    pub seconds: u64,
    pub nanoseconds: u32,
}

impl Timestamp {
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
}

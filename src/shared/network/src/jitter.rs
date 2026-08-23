use anyhow::{Result, anyhow};
use std::collections::HashMap;

use crate::rtp::RtpPacket;

#[derive(Debug, Clone, Copy)]
pub struct JitterBufferConfig {
    pub payload_type: u8,
    pub ssrc: Option<u32>,
    pub frames_per_packet: u32,
    pub capacity_packets: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertResult {
    Accepted,
    Duplicate,
    Late,
    DroppedFull,
}

#[derive(Debug, Clone)]
pub enum PlayoutPacket {
    Packet(RtpPacket),
    Silence {
        sequence_number: u16,
        timestamp: u32,
        frames: u32,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JitterBufferStats {
    pub accepted_packets: u64,
    pub duplicate_packets: u64,
    pub late_packets: u64,
    pub dropped_full_packets: u64,
    pub played_packets: u64,
    pub lost_packets: u64,
    pub silence_packets: u64,
    pub timestamp_discontinuities: u64,
}

pub struct RtpJitterBuffer {
    config: JitterBufferConfig,
    packets: HashMap<u16, RtpPacket>,
    expected_sequence: Option<u16>,
    expected_timestamp: Option<u32>,
    locked_ssrc: Option<u32>,
    stats: JitterBufferStats,
}

impl RtpJitterBuffer {
    pub fn new(config: JitterBufferConfig) -> Result<Self> {
        if !(96..=127).contains(&config.payload_type) {
            return Err(anyhow!(
                "L24 payload type must be dynamic, between 96 and 127"
            ));
        }
        if config.frames_per_packet == 0 {
            return Err(anyhow!("frames per packet must be greater than zero"));
        }
        if config.capacity_packets == 0 {
            return Err(anyhow!("jitter buffer capacity must be greater than zero"));
        }

        Ok(Self {
            config,
            packets: HashMap::with_capacity(config.capacity_packets),
            expected_sequence: None,
            expected_timestamp: None,
            locked_ssrc: config.ssrc,
            stats: JitterBufferStats::default(),
        })
    }

    pub fn insert(&mut self, packet: RtpPacket) -> Result<InsertResult> {
        self.validate_packet(&packet)?;

        if self.expected_sequence.is_none() {
            self.expected_sequence = Some(packet.header.sequence_number);
            self.expected_timestamp = Some(packet.header.timestamp);
        }

        let expected_sequence = self
            .expected_sequence
            .expect("expected sequence should be initialized");

        if sequence_less_than(packet.header.sequence_number, expected_sequence) {
            self.stats.late_packets += 1;
            return Ok(InsertResult::Late);
        }

        if self.packets.contains_key(&packet.header.sequence_number) {
            self.stats.duplicate_packets += 1;
            return Ok(InsertResult::Duplicate);
        }

        if self.packets.len() >= self.config.capacity_packets {
            self.stats.dropped_full_packets += 1;
            return Ok(InsertResult::DroppedFull);
        }

        self.packets.insert(packet.header.sequence_number, packet);
        self.stats.accepted_packets += 1;
        Ok(InsertResult::Accepted)
    }

    pub fn pop_next(&mut self) -> Option<PlayoutPacket> {
        let sequence_number = self.expected_sequence?;
        let timestamp = self.expected_timestamp?;

        let packet = match self.packets.remove(&sequence_number) {
            Some(packet) => {
                if packet.header.timestamp != timestamp {
                    self.stats.timestamp_discontinuities += 1;
                    self.expected_timestamp = Some(
                        packet
                            .header
                            .timestamp
                            .wrapping_add(self.config.frames_per_packet),
                    );
                } else {
                    self.advance_expected_timestamp();
                }
                self.advance_expected_sequence();
                self.stats.played_packets += 1;
                return Some(PlayoutPacket::Packet(packet));
            }
            None => PlayoutPacket::Silence {
                sequence_number,
                timestamp,
                frames: self.config.frames_per_packet,
            },
        };

        self.advance_expected_sequence();
        self.advance_expected_timestamp();
        self.stats.lost_packets += 1;
        self.stats.silence_packets += 1;
        Some(packet)
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn get_stats(&self) -> JitterBufferStats {
        self.stats
    }

    pub fn locked_ssrc(&self) -> Option<u32> {
        self.locked_ssrc
    }

    fn validate_packet(&mut self, packet: &RtpPacket) -> Result<()> {
        if packet.header.payload_type != self.config.payload_type {
            return Err(anyhow!(
                "unexpected RTP payload type {}; expected {}",
                packet.header.payload_type,
                self.config.payload_type
            ));
        }

        match self.locked_ssrc {
            Some(ssrc) if ssrc != packet.header.ssrc => Err(anyhow!(
                "unexpected RTP SSRC 0x{:08X}; expected 0x{ssrc:08X}",
                packet.header.ssrc
            )),
            Some(_) => Ok(()),
            None => {
                self.locked_ssrc = Some(packet.header.ssrc);
                Ok(())
            }
        }
    }

    fn advance_expected_sequence(&mut self) {
        self.expected_sequence = self
            .expected_sequence
            .map(|sequence| sequence.wrapping_add(1));
    }

    fn advance_expected_timestamp(&mut self) {
        self.expected_timestamp = self
            .expected_timestamp
            .map(|timestamp| timestamp.wrapping_add(self.config.frames_per_packet));
    }
}

fn sequence_less_than(left: u16, right: u16) -> bool {
    let diff = left.wrapping_sub(right);
    diff != 0 && diff > 0x8000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::RtpHeader;

    #[test]
    fn in_order_packets_play_out_in_order() {
        let mut buffer = test_buffer();

        assert_eq!(
            buffer.insert(packet(10, 1000)).unwrap(),
            InsertResult::Accepted
        );
        assert_eq!(
            buffer.insert(packet(11, 1048)).unwrap(),
            InsertResult::Accepted
        );

        assert_packet(buffer.pop_next(), 10, 1000);
        assert_packet(buffer.pop_next(), 11, 1048);
        assert_eq!(buffer.get_stats().lost_packets, 0);
    }

    #[test]
    fn reordered_packets_are_recovered() {
        let mut buffer = test_buffer();

        buffer.insert(packet(10, 1000)).unwrap();
        buffer.insert(packet(12, 1096)).unwrap();
        buffer.insert(packet(11, 1048)).unwrap();

        assert_packet(buffer.pop_next(), 10, 1000);
        assert_packet(buffer.pop_next(), 11, 1048);
        assert_packet(buffer.pop_next(), 12, 1096);
        assert_eq!(buffer.get_stats().lost_packets, 0);
    }

    #[test]
    fn duplicate_packets_are_ignored() {
        let mut buffer = test_buffer();

        assert_eq!(
            buffer.insert(packet(10, 1000)).unwrap(),
            InsertResult::Accepted
        );
        assert_eq!(
            buffer.insert(packet(10, 1000)).unwrap(),
            InsertResult::Duplicate
        );

        assert_packet(buffer.pop_next(), 10, 1000);
        assert_eq!(buffer.get_stats().duplicate_packets, 1);
    }

    #[test]
    fn missing_packet_outputs_silence() {
        let mut buffer = test_buffer();

        buffer.insert(packet(10, 1000)).unwrap();
        buffer.insert(packet(12, 1096)).unwrap();

        assert_packet(buffer.pop_next(), 10, 1000);
        assert_silence(buffer.pop_next(), 11, 1048);
        assert_packet(buffer.pop_next(), 12, 1096);
        assert_eq!(buffer.get_stats().lost_packets, 1);
        assert_eq!(buffer.get_stats().silence_packets, 1);
    }

    #[test]
    fn late_packet_is_dropped() {
        let mut buffer = test_buffer();

        buffer.insert(packet(10, 1000)).unwrap();
        assert_packet(buffer.pop_next(), 10, 1000);

        assert_eq!(buffer.insert(packet(10, 1000)).unwrap(), InsertResult::Late);
        assert_eq!(buffer.get_stats().late_packets, 1);
    }

    #[test]
    fn sequence_number_wraps_correctly() {
        let mut buffer = test_buffer();

        buffer.insert(packet(u16::MAX, 1000)).unwrap();
        buffer.insert(packet(0, 1048)).unwrap();

        assert_packet(buffer.pop_next(), u16::MAX, 1000);
        assert_packet(buffer.pop_next(), 0, 1048);
    }

    #[test]
    fn timestamp_gap_is_reported() {
        let mut buffer = test_buffer();

        buffer.insert(packet(10, 1000)).unwrap();
        buffer.insert(packet(11, 2000)).unwrap();

        assert_packet(buffer.pop_next(), 10, 1000);
        assert_packet(buffer.pop_next(), 11, 2000);
        assert_eq!(buffer.get_stats().timestamp_discontinuities, 1);
    }

    #[test]
    fn payload_type_mismatch_is_rejected() {
        let mut buffer = test_buffer();
        let mut packet = packet(10, 1000);
        packet.header.payload_type = 98;

        assert!(buffer.insert(packet).is_err());
    }

    #[test]
    fn static_payload_type_config_is_rejected_for_l24() {
        let result = RtpJitterBuffer::new(JitterBufferConfig {
            payload_type: 95,
            ssrc: None,
            frames_per_packet: 48,
            capacity_packets: 16,
        });

        assert!(result.is_err());
    }

    #[test]
    fn ssrc_mismatch_is_rejected_after_lock() {
        let mut buffer = test_buffer();

        buffer.insert(packet(10, 1000)).unwrap();
        let mut next = packet(11, 1048);
        next.header.ssrc = 0x87654321;

        assert!(buffer.insert(next).is_err());
        assert_eq!(buffer.locked_ssrc(), Some(0x12345678));
    }

    fn test_buffer() -> RtpJitterBuffer {
        RtpJitterBuffer::new(JitterBufferConfig {
            payload_type: 97,
            ssrc: None,
            frames_per_packet: 48,
            capacity_packets: 16,
        })
        .expect("test jitter buffer should be valid")
    }

    fn packet(sequence_number: u16, timestamp: u32) -> RtpPacket {
        RtpPacket {
            header: RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                csrc_count: 0,
                marker: false,
                payload_type: 97,
                sequence_number,
                timestamp,
                ssrc: 0x12345678,
            },
            payload: vec![sequence_number as u8],
        }
    }

    fn assert_packet(output: Option<PlayoutPacket>, sequence_number: u16, timestamp: u32) {
        match output {
            Some(PlayoutPacket::Packet(packet)) => {
                assert_eq!(packet.header.sequence_number, sequence_number);
                assert_eq!(packet.header.timestamp, timestamp);
            }
            other => panic!("expected packet {sequence_number}, got {other:?}"),
        }
    }

    fn assert_silence(output: Option<PlayoutPacket>, sequence_number: u16, timestamp: u32) {
        match output {
            Some(PlayoutPacket::Silence {
                sequence_number: actual_sequence_number,
                timestamp: actual_timestamp,
                frames,
            }) => {
                assert_eq!(actual_sequence_number, sequence_number);
                assert_eq!(actual_timestamp, timestamp);
                assert_eq!(frames, 48);
            }
            other => panic!("expected silence {sequence_number}, got {other:?}"),
        }
    }
}

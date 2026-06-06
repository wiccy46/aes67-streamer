use anyhow::{anyhow, Result};
use audio::AudioSample;

const RTP_FIXED_HEADER_LEN: usize = 12;
const L24_BYTES_PER_SAMPLE: usize = 3;
const L24_POSITIVE_SCALE: f32 = 8_388_607.0;
const L24_NEGATIVE_SCALE: f32 = 8_388_608.0;

/// RTP packet structure for AES67 audio streaming
#[derive(Debug, Clone)]
pub struct RtpPacket {
    /// RTP header (12 bytes minimum)
    pub header: RtpHeader,
    /// Audio payload data
    pub payload: Vec<u8>,
}

/// RTP header structure (RFC 3550)
#[derive(Debug, Clone)]
pub struct RtpHeader {
    /// Version (2 bits): always 2 for RTP
    pub version: u8,
    /// Padding (1 bit): indicates if padding is used
    pub padding: bool,
    /// Extension (1 bit): indicates if header extension is present
    pub extension: bool,
    /// CSRC count (4 bits): number of contributing sources
    pub csrc_count: u8,
    /// Marker (1 bit): profile-specific marker bit
    pub marker: bool,
    /// Payload type (7 bits): audio format identifier
    pub payload_type: u8,
    /// Sequence number (16 bits): increments by 1 for each packet
    pub sequence_number: u16,
    /// Timestamp (32 bits): sampling instant of first octet in payload
    pub timestamp: u32,
    /// SSRC (32 bits): synchronization source identifier
    pub ssrc: u32,
}

impl RtpHeader {
    /// Create new RTP header with defaults for AES67
    pub fn new(payload_type: u8, ssrc: u32) -> Self {
        Self {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type,
            sequence_number: 0,
            timestamp: 0,
            ssrc,
        }
    }

    /// Serialize header to 12-byte array
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut bytes = [0u8; 12];

        // First byte: V(2) + P(1) + X(1) + CC(4)
        bytes[0] = (self.version << 6)
            | ((self.padding as u8) << 5)
            | ((self.extension as u8) << 4)
            | self.csrc_count;

        // Second byte: M(1) + PT(7)
        bytes[1] = ((self.marker as u8) << 7) | self.payload_type;

        // Sequence number (big-endian)
        bytes[2..4].copy_from_slice(&self.sequence_number.to_be_bytes());

        // Timestamp (big-endian)
        bytes[4..8].copy_from_slice(&self.timestamp.to_be_bytes());

        // SSRC (big-endian)
        bytes[8..12].copy_from_slice(&self.ssrc.to_be_bytes());

        bytes
    }

    /// Parse and validate the fixed 12-byte RTP header used by AES67 L24 streams.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < RTP_FIXED_HEADER_LEN {
            return Err(anyhow!(
                "RTP packet is too short: {} bytes, expected at least {RTP_FIXED_HEADER_LEN}",
                bytes.len()
            ));
        }

        let version = bytes[0] >> 6;
        if version != 2 {
            return Err(anyhow!("unsupported RTP version {version}; expected 2"));
        }

        let padding = bytes[0] & 0x20 != 0;
        if padding {
            return Err(anyhow!("RTP padding is not supported"));
        }

        let extension = bytes[0] & 0x10 != 0;
        if extension {
            return Err(anyhow!("RTP header extensions are not supported"));
        }

        let csrc_count = bytes[0] & 0x0f;
        if csrc_count != 0 {
            return Err(anyhow!("RTP CSRC lists are not supported"));
        }

        Ok(Self {
            version,
            padding,
            extension,
            csrc_count,
            marker: bytes[1] & 0x80 != 0,
            payload_type: bytes[1] & 0x7f,
            sequence_number: u16::from_be_bytes([bytes[2], bytes[3]]),
            timestamp: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            ssrc: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        })
    }
}

impl RtpPacket {
    /// Parse an RTP packet into the fixed AES67 header and owned payload bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = RtpHeader::from_bytes(bytes)?;

        Ok(Self {
            header,
            payload: bytes[RTP_FIXED_HEADER_LEN..].to_vec(),
        })
    }
}

/// Decode an AES67 L24 RTP payload into interleaved f32 samples.
pub fn decode_l24_payload_interleaved(
    payload: &[u8],
    channels: u16,
    output: &mut Vec<f32>,
) -> Result<usize> {
    if channels == 0 {
        return Err(anyhow!("channel count must be greater than zero"));
    }

    let bytes_per_frame = channels as usize * L24_BYTES_PER_SAMPLE;
    if !payload.len().is_multiple_of(bytes_per_frame) {
        return Err(anyhow!(
            "L24 payload length {} is not divisible by frame size {bytes_per_frame}",
            payload.len()
        ));
    }

    output.clear();
    output.reserve(payload.len() / L24_BYTES_PER_SAMPLE);

    for sample_bytes in payload.chunks_exact(L24_BYTES_PER_SAMPLE) {
        output.push(decode_l24_sample(sample_bytes));
    }

    Ok(payload.len() / bytes_per_frame)
}

fn decode_l24_sample(bytes: &[u8]) -> f32 {
    let sign = if bytes[0] & 0x80 != 0 { 0xff } else { 0x00 };
    let sample = i32::from_be_bytes([sign, bytes[0], bytes[1], bytes[2]]);

    if sample >= 0 {
        sample as f32 / L24_POSITIVE_SCALE
    } else {
        sample as f32 / L24_NEGATIVE_SCALE
    }
}

fn encode_l24_sample(sample: f32) -> [u8; L24_BYTES_PER_SAMPLE] {
    let sample_f32 = sample.clamp(-1.0, 1.0);
    let scale = if sample_f32 < 0.0 {
        L24_NEGATIVE_SCALE
    } else {
        L24_POSITIVE_SCALE
    };
    let sample_i32 = (sample_f32 * scale) as i32;
    let bytes = sample_i32.to_be_bytes();

    [bytes[1], bytes[2], bytes[3]]
}

/// RTP packet builder for audio samples
pub struct RtpPacketizer {
    /// Current sequence number
    sequence_number: u16,
    /// Base timestamp
    base_timestamp: u32,
    /// Samples processed (for timestamp calculation)
    samples_processed: u64,
    /// RTP header template
    header_template: RtpHeader,
}

impl RtpPacketizer {
    /// Create new RTP packetizer
    pub fn new(payload_type: u8, ssrc: u32) -> Self {
        Self {
            sequence_number: 0,
            base_timestamp: 0,
            samples_processed: 0,
            header_template: RtpHeader::new(payload_type, ssrc),
        }
    }

    /// Set base timestamp (typically from PTP clock)
    pub fn set_base_timestamp(&mut self, timestamp: u32) {
        self.base_timestamp = timestamp;
    }

    /// Create RTP packet from audio sample with explicit timestamp
    pub fn create_packet_with_timestamp(
        &mut self,
        sample: &AudioSample,
        timestamp: u32,
    ) -> Result<RtpPacket> {
        self.create_packet_at_timestamp(sample, timestamp)
    }

    /// Create RTP packet from audio sample
    pub fn create_packet(&mut self, sample: &AudioSample) -> Result<RtpPacket> {
        let timestamp = self.base_timestamp + (self.samples_processed as u32);
        self.create_packet_at_timestamp(sample, timestamp)
    }

    /// Write a serialized RTP packet into an existing buffer.
    pub fn write_packet_into(&mut self, sample: &AudioSample, output: &mut Vec<u8>) -> Result<()> {
        let timestamp = self.base_timestamp + (self.samples_processed as u32);
        self.write_packet_at_timestamp_into(sample, timestamp, output)
    }

    /// Write a serialized RTP packet with an explicit timestamp into an existing buffer.
    pub fn write_packet_with_timestamp_into(
        &mut self,
        sample: &AudioSample,
        timestamp: u32,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        self.write_packet_at_timestamp_into(sample, timestamp, output)
    }

    fn create_packet_at_timestamp(
        &mut self,
        sample: &AudioSample,
        timestamp: u32,
    ) -> Result<RtpPacket> {
        let mut header = self.header_template.clone();
        header.sequence_number = self.sequence_number;
        header.timestamp = timestamp;
        let payload = self.audio_to_payload(sample)?;

        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.samples_processed += sample.frames as u64;

        Ok(RtpPacket { header, payload })
    }

    fn write_packet_at_timestamp_into(
        &mut self,
        sample: &AudioSample,
        timestamp: u32,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let mut header = self.header_template.clone();
        header.sequence_number = self.sequence_number;
        header.timestamp = timestamp;

        output.clear();
        output.reserve(RTP_FIXED_HEADER_LEN + sample.data.len() * L24_BYTES_PER_SAMPLE);
        output.extend_from_slice(&header.to_bytes());
        self.write_audio_payload_into(sample, output)?;

        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.samples_processed += sample.frames as u64;

        Ok(())
    }

    fn audio_to_payload(&self, sample: &AudioSample) -> Result<Vec<u8>> {
        // Convert f32 samples to 24-bit PCM (AES67 standard)
        // Also convert from non-interleaved to interleaved format [L, R, L, R...]
        let mut payload = Vec::with_capacity(sample.data.len() * L24_BYTES_PER_SAMPLE);
        self.write_audio_payload_into(sample, &mut payload)?;

        Ok(payload)
    }

    fn write_audio_payload_into(&self, sample: &AudioSample, payload: &mut Vec<u8>) -> Result<()> {
        let channels = sample.channels as usize;
        let frames = sample.frames;

        for frame_idx in 0..frames {
            for ch_idx in 0..channels {
                let sample_idx = ch_idx * frames + frame_idx;
                if sample_idx < sample.data.len() {
                    payload.extend_from_slice(&encode_l24_sample(sample.data[sample_idx]));
                }
            }
        }

        Ok(())
    }

    /// Get current sequence number
    pub fn get_sequence_number(&self) -> u16 {
        self.sequence_number
    }

    /// Get samples processed count
    pub fn get_samples_processed(&self) -> u64 {
        self.samples_processed
    }

    /// Reset packetizer state
    pub fn reset(&mut self) {
        self.sequence_number = 0;
        self.samples_processed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::AudioSample;

    #[test]
    fn test_rtp_header_serialization() {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: 97,
            sequence_number: 0x1234,
            timestamp: 0x56789ABC,
            ssrc: 0xDEADBEEF,
        };

        let bytes = header.to_bytes();

        // Check version and flags
        assert_eq!(bytes[0], 0x80); // Version=2, P=0, X=0, CC=0
        assert_eq!(bytes[1], 97); // M=0, PT=97

        // Check sequence number
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 0x1234);

        // Check timestamp
        assert_eq!(
            u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            0x56789ABC
        );

        // Check SSRC
        assert_eq!(
            u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            0xDEADBEEF
        );
    }

    #[test]
    fn rtp_header_parses_serialized_header() {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: true,
            payload_type: 97,
            sequence_number: 0x1234,
            timestamp: 0x56789ABC,
            ssrc: 0xDEADBEEF,
        };

        let parsed = RtpHeader::from_bytes(&header.to_bytes()).unwrap();

        assert_eq!(parsed.version, 2);
        assert!(parsed.marker);
        assert_eq!(parsed.payload_type, 97);
        assert_eq!(parsed.sequence_number, 0x1234);
        assert_eq!(parsed.timestamp, 0x56789ABC);
        assert_eq!(parsed.ssrc, 0xDEADBEEF);
    }

    #[test]
    fn rtp_packet_parser_accepts_packetizer_bytes() {
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let mut bytes = Vec::new();

        packetizer
            .write_packet_with_timestamp_into(&sample, 0xABCDEF01, &mut bytes)
            .unwrap();

        let parsed = RtpPacket::parse(&bytes).unwrap();

        assert_eq!(parsed.header.version, 2);
        assert_eq!(parsed.header.payload_type, 97);
        assert_eq!(parsed.header.sequence_number, 0);
        assert_eq!(parsed.header.timestamp, 0xABCDEF01);
        assert_eq!(parsed.header.ssrc, 0x12345678);
        assert_eq!(parsed.payload, bytes[RTP_FIXED_HEADER_LEN..]);
    }

    #[test]
    fn rtp_packet_parser_rejects_short_packets() {
        let result = RtpPacket::parse(&[0x80, 97, 0, 1]);

        assert!(result.is_err());
    }

    #[test]
    fn rtp_packet_parser_rejects_non_version_two_packets() {
        let mut bytes = [0u8; RTP_FIXED_HEADER_LEN];
        bytes[0] = 0x40;

        let result = RtpPacket::parse(&bytes);

        assert!(result.is_err());
    }

    #[test]
    fn rtp_packet_parser_rejects_unsupported_header_features() {
        let mut padding = RtpHeader::new(97, 0x12345678).to_bytes();
        padding[0] |= 0x20;
        assert!(RtpPacket::parse(&padding).is_err());

        let mut extension = RtpHeader::new(97, 0x12345678).to_bytes();
        extension[0] |= 0x10;
        assert!(RtpPacket::parse(&extension).is_err());

        let mut csrc = RtpHeader::new(97, 0x12345678).to_bytes();
        csrc[0] |= 0x01;
        assert!(RtpPacket::parse(&csrc).is_err());
    }

    #[test]
    fn test_rtp_packetizer() {
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);

        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };

        let packet = packetizer.create_packet(&sample).unwrap();

        // Check header
        assert_eq!(packet.header.sequence_number, 0);
        assert_eq!(packet.header.payload_type, 97);
        assert_eq!(packet.header.ssrc, 0x12345678);

        // Check payload size (4 samples * 3 bytes each = 12 bytes)
        assert_eq!(packet.payload.len(), 12);

        // Create second packet
        let packet2 = packetizer.create_packet(&sample).unwrap();
        assert_eq!(packet2.header.sequence_number, 1);
        assert_eq!(packet2.header.timestamp, 2); // 2 frames processed
    }

    #[test]
    fn explicit_timestamp_packets_still_advance_packetizer_state() {
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };

        let packet = packetizer
            .create_packet_with_timestamp(&sample, 0xABCDEF01)
            .unwrap();

        assert_eq!(packet.header.timestamp, 0xABCDEF01);
        assert_eq!(packet.header.sequence_number, 0);
        assert_eq!(packetizer.get_sequence_number(), 1);
        assert_eq!(packetizer.get_samples_processed(), 2);
    }

    #[test]
    fn packet_bytes_writer_matches_existing_packet_serialization() {
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let packet = packetizer
            .create_packet_with_timestamp(&sample, 0xABCDEF01)
            .unwrap();
        let mut expected = packet.header.to_bytes().to_vec();
        expected.extend(packet.payload);

        let mut writer = RtpPacketizer::new(97, 0x12345678);
        let mut actual = Vec::new();
        writer
            .write_packet_with_timestamp_into(&sample, 0xABCDEF01, &mut actual)
            .unwrap();

        assert_eq!(actual, expected);
        assert_eq!(writer.get_sequence_number(), 1);
        assert_eq!(writer.get_samples_processed(), 2);
    }

    #[test]
    fn packet_bytes_writer_advances_timestamps_by_sample_frames() {
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        packetizer.set_base_timestamp(0xABCDEF00);
        let mut bytes = Vec::new();

        packetizer.write_packet_into(&sample, &mut bytes).unwrap();
        let first = RtpPacket::parse(&bytes).unwrap();

        packetizer.write_packet_into(&sample, &mut bytes).unwrap();
        let second = RtpPacket::parse(&bytes).unwrap();

        assert_eq!(first.header.timestamp, 0xABCDEF00);
        assert_eq!(second.header.timestamp, 0xABCDEF02);
        assert_eq!(second.header.sequence_number, 1);
    }

    #[test]
    fn packet_bytes_writer_reuses_existing_buffer_capacity() {
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        let expected_packet_len = 12 + sample.data.len() * 3;
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let mut output = Vec::with_capacity(expected_packet_len);

        packetizer.write_packet_into(&sample, &mut output).unwrap();
        let first_ptr = output.as_ptr();
        let first_capacity = output.capacity();
        assert_eq!(output.len(), expected_packet_len);

        packetizer.write_packet_into(&sample, &mut output).unwrap();

        assert_eq!(output.as_ptr(), first_ptr);
        assert_eq!(output.capacity(), first_capacity);
        assert_eq!(output.len(), expected_packet_len);
        assert_eq!(packetizer.get_sequence_number(), 2);
        assert_eq!(packetizer.get_samples_processed(), 4);
    }

    #[test]
    fn reset_clears_sequence_and_sample_position() {
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let sample = AudioSample {
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        packetizer.create_packet(&sample).unwrap();

        packetizer.reset();

        assert_eq!(packetizer.get_sequence_number(), 0);
        assert_eq!(packetizer.get_samples_processed(), 0);
    }

    #[test]
    fn test_audio_to_payload_conversion() {
        let packetizer = RtpPacketizer::new(97, 0);

        let sample = AudioSample {
            data: vec![1.0, -1.0, 0.0],
            channels: 1,
            sample_rate: 48000,
            frames: 3,
        };

        let payload = packetizer.audio_to_payload(&sample).unwrap();

        // Should be 9 bytes (3 samples * 3 bytes each)
        assert_eq!(payload.len(), 9);

        // Check 24-bit conversion
        // 1.0 -> 0x7FFFFF (max positive)
        assert_eq!(&payload[0..3], &[0x7F, 0xFF, 0xFF]);

        // -1.0 -> 0x800000 (max negative in 24-bit two's complement)
        assert_eq!(&payload[3..6], &[0x80, 0x00, 0x00]);

        // 0.0 -> 0x000000
        assert_eq!(&payload[6..9], &[0x00, 0x00, 0x00]);
    }

    #[test]
    fn l24_payload_decoder_handles_canonical_boundaries() {
        let payload = [
            0x7F, 0xFF, 0xFF, // max positive
            0x80, 0x00, 0x00, // max negative
            0x00, 0x00, 0x00, // zero
        ];
        let mut decoded = Vec::new();

        let frames = decode_l24_payload_interleaved(&payload, 1, &mut decoded).unwrap();

        assert_eq!(frames, 3);
        assert_close(decoded[0], 1.0);
        assert_close(decoded[1], -1.0);
        assert_close(decoded[2], 0.0);
    }

    #[test]
    fn l24_payload_decoder_preserves_interleaved_order_from_packetizer() {
        let sample = AudioSample {
            // Planar input: left frame 0, left frame 1, right frame 0, right frame 1.
            data: vec![0.5, -0.5, 0.25, -0.25],
            channels: 2,
            sample_rate: 48000,
            frames: 2,
        };
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let packet = packetizer.create_packet(&sample).unwrap();
        let mut decoded = Vec::new();

        let frames =
            decode_l24_payload_interleaved(&packet.payload, sample.channels as u16, &mut decoded)
                .unwrap();

        assert_eq!(frames, 2);
        assert_eq!(decoded.len(), 4);
        assert_close(decoded[0], 0.5);
        assert_close(decoded[1], 0.25);
        assert_close(decoded[2], -0.5);
        assert_close(decoded[3], -0.25);
    }

    #[test]
    fn packetize_parse_decode_preserves_sine_continuity_across_packet_boundaries() {
        let sample_rate = 48_000;
        let channels = 2;
        let frames_per_packet = 48;
        let packet_count = 10;
        let frequency_hz = 997.0;
        let amplitude = 0.5;
        let mut packetizer = RtpPacketizer::new(97, 0x12345678);
        let mut decoded_stream = Vec::new();
        let mut expected_interleaved = Vec::new();

        for packet_index in 0..packet_count {
            let frame_offset = packet_index * frames_per_packet;
            let packet_frames = (0..frames_per_packet)
                .map(|frame| {
                    sine_sample(frame_offset + frame, sample_rate, frequency_hz, amplitude)
                })
                .collect::<Vec<_>>();
            let mut planar = packet_frames.clone();
            planar.extend_from_slice(&packet_frames);

            for sample in &packet_frames {
                expected_interleaved.push(*sample);
                expected_interleaved.push(*sample);
            }

            let sample = AudioSample {
                data: planar,
                channels,
                sample_rate,
                frames: frames_per_packet,
            };
            let mut bytes = Vec::new();
            packetizer
                .write_packet_into(&sample, &mut bytes)
                .expect("packetizer should serialize sine packet");

            let packet = RtpPacket::parse(&bytes).expect("serialized packet should parse");
            let mut decoded_packet = Vec::new();
            let frames = decode_l24_payload_interleaved(
                &packet.payload,
                channels as u16,
                &mut decoded_packet,
            )
            .expect("L24 packet should decode");

            assert_eq!(frames, frames_per_packet);
            decoded_stream.extend(decoded_packet);
        }

        assert_eq!(decoded_stream.len(), expected_interleaved.len());
        for (actual, expected) in decoded_stream.iter().zip(expected_interleaved.iter()) {
            assert_close(*actual, *expected);
        }

        let max_jump = decoded_stream
            .chunks_exact(channels as usize)
            .map(|frame| frame[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|window| (window[1] - window[0]).abs())
            .fold(0.0f32, f32::max);

        assert!(
            max_jump < 0.07,
            "decoded sine has an unexpected adjacent-frame jump: {max_jump}"
        );
    }

    #[test]
    fn l24_payload_decoder_rejects_invalid_payload_shape() {
        let mut decoded = Vec::new();

        assert!(decode_l24_payload_interleaved(&[0x00, 0x00], 1, &mut decoded).is_err());
        assert!(decode_l24_payload_interleaved(&[0x00, 0x00, 0x00], 0, &mut decoded).is_err());
    }

    #[test]
    fn eight_channel_payload_is_frame_interleaved() {
        let packetizer = RtpPacketizer::new(97, 0);
        let sample_values = (1..=16)
            .map(|value| value as f32 / 8_388_607.0)
            .collect::<Vec<_>>();
        let sample = AudioSample {
            // Planar input: ch1 frame1, ch1 frame2, ch2 frame1, ch2 frame2, ...
            data: sample_values.clone(),
            channels: 8,
            sample_rate: 48000,
            frames: 2,
        };

        let payload = packetizer.audio_to_payload(&sample).unwrap();
        let expected_indices = [0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15];
        let expected = expected_indices
            .iter()
            .flat_map(|&index| pcm24_bytes(sample_values[index]))
            .collect::<Vec<_>>();

        assert_eq!(payload, expected);
    }

    fn pcm24_bytes(sample: f32) -> [u8; 3] {
        encode_l24_sample(sample)
    }

    fn assert_close(actual: f32, expected: f32) {
        let tolerance = 2.0 / L24_POSITIVE_SCALE;
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual {actual} expected {expected}"
        );
    }

    fn sine_sample(frame: usize, sample_rate: u32, frequency_hz: f32, amplitude: f32) -> f32 {
        (2.0 * std::f32::consts::PI * frequency_hz * frame as f32 / sample_rate as f32).sin()
            * amplitude
    }
}

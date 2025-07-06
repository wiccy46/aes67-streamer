#!/usr/bin/env python3
"""
AES67 Stream Validator
Analyzes captured RTP packets for AES67 compliance
"""

import struct
import socket
import time
import argparse
from dataclasses import dataclass
from typing import List, Optional

@dataclass
class RtpPacket:
    version: int
    padding: bool
    extension: bool
    csrc_count: int
    marker: bool
    payload_type: int
    sequence_number: int
    timestamp: int
    ssrc: int
    payload: bytes

class Aes67Validator:
    def __init__(self):
        self.packets: List[RtpPacket] = []
        self.errors: List[str] = []
        self.warnings: List[str] = []
    
    def parse_rtp_packet(self, data: bytes) -> Optional[RtpPacket]:
        """Parse RTP packet from raw bytes"""
        if len(data) < 12:
            self.errors.append(f"Packet too short: {len(data)} bytes")
            return None
        
        # Parse RTP header (RFC 3550)
        header = struct.unpack('!BBHII', data[:12])
        
        # Extract fields from first byte
        version = (header[0] >> 6) & 0x3
        padding = bool((header[0] >> 5) & 0x1)
        extension = bool((header[0] >> 4) & 0x1)
        csrc_count = header[0] & 0xF
        
        # Extract fields from second byte
        marker = bool((header[1] >> 7) & 0x1)
        payload_type = header[1] & 0x7F
        
        sequence_number = header[2]
        timestamp = header[3]
        ssrc = header[4]
        
        # Extract payload (skip CSRC if present)
        payload_start = 12 + (csrc_count * 4)
        payload = data[payload_start:]
        
        return RtpPacket(
            version=version,
            padding=padding,
            extension=extension,
            csrc_count=csrc_count,
            marker=marker,
            payload_type=payload_type,
            sequence_number=sequence_number,
            timestamp=timestamp,
            ssrc=ssrc,
            payload=payload
        )
    
    def validate_aes67_compliance(self) -> bool:
        """Validate captured packets against AES67 requirements"""
        if not self.packets:
            self.errors.append("No packets to validate")
            return False
        
        print(f"🔍 Validating {len(self.packets)} RTP packets for AES67 compliance...")
        print()
        
        # AES67 Requirement Checks
        self._check_rtp_version()
        self._check_payload_type()
        self._check_sequence_numbers()
        self._check_timestamp_increment()
        self._check_packet_timing()
        self._check_audio_format()
        
        # Summary
        print(f"📊 Validation Summary:")
        print(f"  Errors: {len(self.errors)}")
        print(f"  Warnings: {len(self.warnings)}")
        
        if self.errors:
            print(f"❌ AES67 Compliance: FAILED")
            for error in self.errors:
                print(f"    ❌ {error}")
        else:
            print(f"✅ AES67 Compliance: PASSED")
        
        if self.warnings:
            print(f"⚠️  Warnings:")
            for warning in self.warnings:
                print(f"    ⚠️  {warning}")
        
        return len(self.errors) == 0
    
    def _check_rtp_version(self):
        """Check RTP version is 2"""
        for i, packet in enumerate(self.packets):
            if packet.version != 2:
                self.errors.append(f"Packet {i}: Invalid RTP version {packet.version} (should be 2)")
    
    def _check_payload_type(self):
        """Check payload type is appropriate for AES67"""
        pt_counts = {}
        for packet in self.packets:
            pt_counts[packet.payload_type] = pt_counts.get(packet.payload_type, 0) + 1
        
        print(f"📋 Payload Types: {pt_counts}")
        
        # AES67 typically uses dynamic payload types (96-127)
        for pt in pt_counts:
            if pt < 96:
                self.warnings.append(f"Payload type {pt} is not dynamic (AES67 recommends 96-127)")
    
    def _check_sequence_numbers(self):
        """Check RTP sequence numbers increment correctly"""
        if len(self.packets) < 2:
            return
        
        gaps = 0
        duplicates = 0
        
        for i in range(1, len(self.packets)):
            prev_seq = self.packets[i-1].sequence_number
            curr_seq = self.packets[i].sequence_number
            
            expected = (prev_seq + 1) % 65536
            if curr_seq != expected:
                if curr_seq == prev_seq:
                    duplicates += 1
                else:
                    gaps += 1
        
        print(f"📈 Sequence Analysis:")
        print(f"  Packets: {len(self.packets)}")
        print(f"  Gaps: {gaps}")
        print(f"  Duplicates: {duplicates}")
        
        if gaps > 0:
            self.warnings.append(f"Found {gaps} sequence number gaps (possible packet loss)")
        if duplicates > 0:
            self.errors.append(f"Found {duplicates} duplicate sequence numbers")
    
    def _check_timestamp_increment(self):
        """Check timestamp increments are consistent"""
        if len(self.packets) < 3:
            return
        
        increments = []
        for i in range(1, len(self.packets)):
            prev_ts = self.packets[i-1].timestamp
            curr_ts = self.packets[i].timestamp
            
            # Handle timestamp wraparound
            if curr_ts >= prev_ts:
                increment = curr_ts - prev_ts
            else:
                increment = (2**32 - prev_ts) + curr_ts
            
            increments.append(increment)
        
        if increments:
            avg_increment = sum(increments) / len(increments)
            print(f"⏱️  Timestamp Analysis:")
            print(f"  Average increment: {avg_increment:.1f}")
            print(f"  Expected for 48kHz 1ms: 48")
            
            # Check if increment suggests 48kHz with 1ms packets
            if abs(avg_increment - 48) > 5:
                self.warnings.append(f"Timestamp increment {avg_increment:.1f} doesn't match 48kHz 1ms packets")
    
    def _check_packet_timing(self):
        """Analyze packet timing for AES67 compliance"""
        print(f"📦 Packet Analysis:")
        print(f"  Total packets: {len(self.packets)}")
        
        if self.packets:
            print(f"  First SSRC: 0x{self.packets[0].ssrc:08x}")
            print(f"  Payload size: {len(self.packets[0].payload)} bytes")
    
    def _check_audio_format(self):
        """Check audio format compliance"""
        if self.packets:
            payload_size = len(self.packets[0].payload)
            
            # AES67 typically uses:
            # - 2 channels × 24-bit × 48 samples = 288 bytes for 1ms at 48kHz
            # - 2 channels × 16-bit × 48 samples = 192 bytes for 1ms at 48kHz
            
            expected_sizes = {
                192: "2ch 16-bit 48kHz 1ms",
                288: "2ch 24-bit 48kHz 1ms", 
                384: "2ch 32-bit 48kHz 1ms",
                96: "1ch 16-bit 48kHz 1ms",
                144: "1ch 24-bit 48kHz 1ms"
            }
            
            print(f"🎵 Audio Format Analysis:")
            print(f"  Payload size: {payload_size} bytes")
            
            if payload_size in expected_sizes:
                print(f"  Format: {expected_sizes[payload_size]}")
            else:
                self.warnings.append(f"Unusual payload size {payload_size} bytes")

def capture_live_stream(multicast_addr: str, port: int, duration: int = 5) -> List[bytes]:
    """Capture live RTP packets from multicast stream"""
    print(f"📡 Capturing live stream: {multicast_addr}:{port} for {duration}s...")
    
    # Create multicast socket
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(('', port))
    
    # Join multicast group
    mreq = struct.pack('4sl', socket.inet_aton(multicast_addr), socket.INADDR_ANY)
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
    
    packets = []
    start_time = time.time()
    
    sock.settimeout(1.0)
    
    try:
        while time.time() - start_time < duration:
            try:
                data, addr = sock.recvfrom(2048)
                packets.append(data)
                if len(packets) % 50 == 0:
                    print(f"  Captured {len(packets)} packets...")
            except socket.timeout:
                continue
    except KeyboardInterrupt:
        print("  Capture interrupted by user")
    finally:
        sock.close()
    
    print(f"✅ Captured {len(packets)} packets")
    return packets

def main():
    parser = argparse.ArgumentParser(description='Validate AES67 RTP stream compliance')
    parser.add_argument('--capture', action='store_true', help='Capture live stream')
    parser.add_argument('--address', default='239.69.83.1', help='Multicast address')
    parser.add_argument('--port', type=int, default=5004, help='Port number')
    parser.add_argument('--duration', type=int, default=5, help='Capture duration (seconds)')
    
    args = parser.parse_args()
    
    validator = Aes67Validator()
    
    if args.capture:
        # Capture live stream
        raw_packets = capture_live_stream(args.address, args.port, args.duration)
        
        # Parse packets
        for raw_packet in raw_packets:
            packet = validator.parse_rtp_packet(raw_packet)
            if packet:
                validator.packets.append(packet)
        
        print()
        validator.validate_aes67_compliance()
    else:
        print("Use --capture to capture and validate live AES67 stream")
        print(f"Example: python3 {__file__} --capture --address 239.69.83.1 --port 5004")

if __name__ == '__main__':
    main()
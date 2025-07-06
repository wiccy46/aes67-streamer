# AES67 Compliance Testing Guide

This guide explains how to validate that the AES67 streamer produces compliant streams that can be detected by professional AES67 monitoring tools.

## 🧪 Testing Tools

### 1. Automated E2E Test Script
```bash
./tests/e2e_aes67_compliance.sh
```
Runs comprehensive tests including network capture and basic compliance checks.

### 2. Python Validator
```bash
python3 tests/aes67_validator.py --capture --address 239.69.83.1 --port 5004 --duration 10
```
Captures live RTP packets and validates against AES67 requirements.

## 🔍 AES67 Stream Monitor Testing

### Prerequisites
1. **Download AES67 Stream Monitor**
   - RAVENNA Stream Monitor (free): [ALC NetworX website](https://www.alcnetworx.com/)
   - Alternative: Dante Controller (Audinate)

2. **Network Setup**
   - Ensure all devices on same network segment
   - Multicast routing enabled
   - Firewall allows multicast traffic

### Testing Procedure

#### Step 1: Start AES67 Streamer
```bash
cargo run --release -- \
  --file tests/piano_freesound.wav \
  --address 239.69.83.1 \
  --port 5004 \
  --interface en0 \
  --verbose
```

#### Step 2: Monitor Detection
1. Open AES67 Stream Monitor
2. Look for stream advertisement
3. Check stream appears in discovered devices
4. Verify audio parameters:
   - Sample Rate: 48kHz
   - Channels: 2 (stereo)
   - Bit Depth: 24-bit
   - Packet Time: 1ms

#### Step 3: Audio Verification
1. Connect to the stream in monitor
2. Verify audio plays correctly
3. Check for dropouts or distortion
4. Monitor PTP synchronization status

## 📊 Compliance Checklist

### ✅ Network Layer (AES67-2018)
- [ ] Multicast address in range 239.69.0.0/16
- [ ] Default port 5004 (or user specified)
- [ ] RTP over UDP transport
- [ ] Proper multicast TTL settings

### ✅ RTP Layer (RFC 3550)
- [ ] RTP version 2
- [ ] Dynamic payload type (96-127)
- [ ] Correct sequence numbering
- [ ] Proper timestamp increments
- [ ] Unique SSRC identifier

### ✅ Audio Format (AES67)
- [ ] Sample rate: 48kHz (default)
- [ ] Packet time: 1ms (48 samples)
- [ ] Bit depths: 16, 24, or 32-bit
- [ ] Channel count: 1-64 channels
- [ ] Linear PCM encoding

### ✅ Timing (IEEE 1588)
- [ ] PTP synchronization enabled
- [ ] PTP domain configuration
- [ ] Timestamp accuracy ±1μs
- [ ] Clock drift compensation

## 🛠️ Troubleshooting

### Stream Not Detected
1. **Check Network Interface**
   ```bash
   ip route get 239.69.83.1  # Linux
   route get 239.69.83.1     # macOS
   ```

2. **Verify Multicast Routing**
   ```bash
   netstat -rn | grep 224    # Check multicast routes
   ```

3. **Test with VLC**
   ```bash
   vlc rtp://@239.69.83.1:5004
   ```

### Audio Issues
1. **Check Sample Rate**
   - Ensure 48kHz for AES67 compatibility
   - Verify resampling is working

2. **Verify Packet Size**
   - 1ms packets = 48 samples
   - 2 channels × 24-bit = 288 bytes payload

3. **Monitor Packet Loss**
   ```bash
   python3 tests/aes67_validator.py --capture
   ```

### PTP Synchronization
1. **Check PTP Master**
   ```bash
   # Look for PTP announce messages
   tcpdump -i en0 port 319
   ```

2. **Verify Clock Accuracy**
   - Use PTP-aware test equipment
   - Check timestamp consistency

## 📈 Performance Validation

### Latency Testing
```bash
# Measure end-to-end latency with loopback
# Generator -> AES67 Stream -> Receiver -> Analysis
```

### Jitter Analysis
```bash
# Capture timing data
tcpdump -i en0 -w timing.pcap host 239.69.83.1
# Analyze with Wireshark RTP stream analysis
```

### Load Testing
```bash
# Multiple concurrent streams
for i in {1..8}; do
  cargo run -- --file tests/piano_freesound.wav \
    --address 239.69.83.$i --port $((5004 + i)) &
done
```

## 🔧 Advanced Testing

### Professional Test Equipment
1. **Audio Analyzers**
   - R&S UPV Audio Analyzer
   - APx Audio Precision analyzers

2. **Network Analyzers**
   - Wireshark with AES67 dissectors
   - SolarWinds Network Performance Monitor

3. **AES67 Test Suites**
   - Merging Technologies test tools
   - Lawo AES67 test applications

### Interoperability Testing
Test with devices from different manufacturers:
- Dante devices (with AES67 mode)
- Livewire+ equipment
- RAVENNA devices
- Networked mixing consoles

## 📝 Test Results Documentation

### Required Measurements
1. **Stream Discovery Time**: < 2 seconds
2. **Audio Latency**: < 10ms typical
3. **Packet Loss**: 0% under normal conditions
4. **PTP Accuracy**: ±1μs from master
5. **Jitter**: < 50μs

### Certification Checklist
- [ ] Stream detected by multiple monitoring tools
- [ ] Audio quality verified by listening test
- [ ] Packet analysis shows compliance
- [ ] PTP synchronization working
- [ ] Interoperability with other AES67 devices
- [ ] Performance under load acceptable

## 🌐 Real-World Testing

### Studio Integration
1. Test with professional audio consoles
2. Verify integration with broadcast systems
3. Check compatibility with existing workflows

### Live Production
1. Test in live streaming environments
2. Verify reliability over extended periods
3. Check behavior during network congestion

This comprehensive testing ensures the AES67 streamer meets professional broadcast standards and works seamlessly with existing AES67 infrastructure.
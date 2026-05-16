# Tests for AES67 Streamer

## Running locally

```bash
cargo test --workspace
bash scripts/e2e_loopback.sh
```

## Test Tiers

| Tier | Command | Purpose | CI |
| --- | --- | --- | --- |
| Unit and integration | `cargo test --workspace` | Fast Rust coverage for audio, config, RTP, PTP, socket behavior, and script dry-run validation | Yes |
| Media loopback | `bash scripts/e2e_loopback.sh` | Full streamer-to-ffmpeg RTP loopback with decoded WAV validation | Yes |
| Multicast validation | `AES67_E2E_INTERFACE=<local-ip> bash scripts/e2e_multicast.sh` | Opt-in multicast group join and receive validation on a real interface | No |
| Soak validation | `bash scripts/soak_loopback.sh` | Longer local release-candidate run using the media loopback path | No |
| Receiver compatibility | `tests/receiver-compatibility.md` | Manual/pro-tool matrix for VLC, RAVENNA Stream Monitor, Dante AES67 mode, and Wireshark | No |

## Full Media Loopback E2E

`scripts/e2e_loopback.sh` is the local and CI entry point for end-to-end media testing. It:

- Builds `aes67-streamer`.
- Generates a short 48 kHz stereo WAV fixture.
- Starts `ffmpeg` as an RTP receiver from an SDP file.
- Runs the streamer for a bounded duration on loopback.
- Validates the recorded WAV with `ffprobe`.

Requirements:

- `ffmpeg`
- `ffprobe`
- Local UDP loopback networking

## Optional Multicast E2E

`scripts/e2e_multicast.sh` validates that ffmpeg can join the selected multicast
group and record the RTP stream. It requires an explicit local interface IPv4:

```bash
AES67_E2E_INTERFACE=192.168.1.100 bash scripts/e2e_multicast.sh
```

Use `--dry-run` to verify the selected address, port, interface, and artifact
directory without starting the streamer:

```bash
AES67_E2E_INTERFACE=192.168.1.100 bash scripts/e2e_multicast.sh --dry-run
```

## Soak Validation

`scripts/soak_loopback.sh` runs the media loopback path for a longer duration.
The default is 300 seconds:

```bash
bash scripts/soak_loopback.sh
```

To change the duration:

```bash
AES67_SOAK_DURATION_SECONDS=900 bash scripts/soak_loopback.sh
```

## CI

GitHub Actions on `ubuntu-latest` runs `cargo test --workspace` and `bash scripts/e2e_loopback.sh`.

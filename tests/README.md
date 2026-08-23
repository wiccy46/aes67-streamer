# AES67 Tools Tests

The only product command under test is `aes67`:

```bash
cargo test --workspace
bash scripts/e2e_loopback.sh
```

| Tier | Command | Purpose | CI |
| --- | --- | --- | --- |
| Unit and integration | `cargo test --workspace` | Audio, network, PTP, configuration, Send, Receive, discovery, and scripts | Yes |
| Send media loopback | `bash scripts/e2e_loopback.sh` | `aes67 send file` to ffmpeg RTP loopback with decoded WAV validation | Yes |
| Receive null-output E2E | `cargo test -p aes67 --test receive_e2e` | Send-to-Receive RTP validation without an audio device | Yes |
| Receive null-output soak | `bash scripts/receive_soak_loopback.sh` | Longer receive, jitter, decode, and counter validation | No |
| Receive CPAL loopback | `bash scripts/receive_cpal_loopback.sh` | Send-to-Receive validation using real CPAL audio output | No |
| Multicast validation | `AES67_E2E_INTERFACE=<local-ip> bash scripts/e2e_multicast.sh` | Opt-in multicast interoperability on a selected interface | No |

## Receive validation

The Receive workflow supports basic arguments and SDP files:

```bash
cargo run -p aes67 -- receive listen \
  --address 239.192.1.1 \
  --port 5004 \
  --interface 127.0.0.1

cargo run -p aes67 -- receive listen \
  --sdp tests/example.sdp \
  --interface 127.0.0.1

cargo run -p aes67 -- receive devices
```

For real playback checks:

```bash
bash scripts/receive_cpal_loopback.sh
AES67_RECEIVE_OUTPUT_DEVICE=<index-or-name> bash scripts/receive_cpal_loopback.sh
```

The CPAL loopback rejects known null/discard devices by default. Use
`AES67_RECEIVE_ALLOW_UNCLOCKED_OUTPUT=1` only for a diagnostic startup check.

For longer CI-safe validation:

```bash
bash scripts/receive_soak_loopback.sh
AES67_RECEIVE_SOAK_DURATION_SECONDS=300 bash scripts/receive_soak_loopback.sh
```

## Discovery validation

SAP discovery has parser, registry, socket, and process-level coverage:

```bash
cargo test -p aes67 --test discovery_e2e
```

## Optional multicast and long-running checks

```bash
AES67_E2E_INTERFACE=192.168.1.100 bash scripts/e2e_multicast.sh
bash scripts/soak_loopback.sh
```

Use `--dry-run` with either script to validate its selected configuration
without sending media.

# Timing And Scheduling Decision

## Release Decision

The first public release keeps packet scheduling portable and best-effort. The
streamer uses Tokio timing and reports measured packet lateness at the end of a
run. It does not claim hard real-time scheduling or sub-millisecond delivery
guarantees.

This is acceptable for the first single-stream release because:

- The packet format, RTP timestamping, SDP/SAP metadata, QoS markings, and PTP
  behavior are now testable.
- Timing drift is visible in final stream statistics instead of hidden.
- CI and local loopback tests validate bounded stream behavior.
- Optional soak testing can measure longer local runs before a release tag.

## What To Check Before Release

Run:

```bash
bash scripts/e2e_loopback.sh
bash scripts/soak_loopback.sh
```

Review the final streamer statistics:

- Packet count should match the configured duration and packet time closely.
- Packet rate should stay near the expected rate:
  - 1 ms packet time: about 1000 packets/second.
  - 2 ms packet time: about 500 packets/second.
- Timing max lateness and average late-packet lateness should be recorded in
  the release notes for the test machine.
- Any "Streaming falling behind" warnings should be investigated before
  release.

## What This Release Does Not Claim

The release does not claim:

- Real-time thread priority.
- Kernel bypass networking.
- Hardware timestamping.
- PTP hardware clock discipline.
- Guaranteed latency under CPU, disk, or network contention.

## Follow-Up Work

Platform-specific scheduling can be added later if measured drift or receiver
testing shows it is necessary. Candidate follow-ups:

- Linux real-time thread priority with documented privileges.
- macOS quality-of-service or thread policy tuning.
- More detailed timing histograms in long-running tests.
- Allocation profiling around the full audio-to-RTP path.

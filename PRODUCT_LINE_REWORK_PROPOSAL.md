# AES67 Product-Line Refactor Proposal

## Decision

Ship one product: `aes67`.

It has two customer-facing functions:

```text
aes67
├── send
│   ├── file       Send one audio file
│   └── queue      Send a managed file queue
└── receive
    ├── discover   Find SAP-announced streams
    ├── listen     Receive one stream to a local output
    └── devices    List local audio outputs
```

`player` is no longer a product name. The network role is **Receive**; local
audio playout is the **listen** action. Discovery belongs to Receive because it
is the first step in joining a stream.

The former route tester is not a third product line. It remains a separate
developer and installer diagnostic until it has a distinct customer workflow.
It is not packaged or exposed by the public `aes67` command.

## Implemented structure

```text
src/
├── aes67/                         # the only shipped application
│   └── src/main.rs                 # CLI adapter and exit-code formatting
│
├── shared/
│   ├── aes67-engine/               # Send and Receive workflow library
│   │   ├── sender/
│   │   │   ├── mod.rs               # file-send operation
│   │   │   ├── engine.rs             # RTP/PTP/SAP send implementation
│   │   │   ├── queue.rs             # queued sender terminal experience
│   │   │   └── runtime.rs
│   │   ├── receiver/
│   │   │   ├── mod.rs               # listen operation and device listing
│   │   │   ├── session.rs            # RTP receive, jitter, decode, playout
│   │   │   ├── output.rs            # CPAL output adapter
│   │   │   └── runtime.rs
│   │   └── discovery.rs             # SAP discovery workflow
│   ├── audio/                       # decode, resample, processing
│   ├── network/                     # RTP, SDP, SAP, UDP, jitter
│   ├── ptp/                         # PTP clock and timing
│   └── config/                      # CLI and TOML parsing
│
└── tools/aes67-route-test/          # non-product route diagnostic
```

Dependency direction:

```text
CLI now / TUI later / Desktop GUI later
                    │
                    ▼
               aes67-engine
          ┌─────────┴─────────┐
          │                   │
        sender             receiver
                              │
                         discovery
          │                   │
          └──── audio / network / ptp ────┘
```

`audio`, `network`, and `ptp` are lower-level reusable libraries. They support
the engine as a whole; they do not belong to a `verify` feature.

## Product boundaries

| Function | Owns | Does not own |
| --- | --- | --- |
| Send | file decode, resampling, RTP transmission, SDP/SAP and PTP send setup | receiver playout or route diagnostics |
| Receive | SAP discovery, session selection, RTP receive, jitter, decode, output-device selection and playout | router control, multi-stream mixing or recording |
| Route diagnostic | known-signal return-path checks for development/installation | the normal Send/Receive application |

## Packaging decision

Release archives and Homebrew install only `aes67`. No companion binaries are
required at runtime because the CLI calls `aes67-engine` directly in-process.

The previous standalone product crates and their compatibility wrappers have
been removed. Send, Receive, Discovery, and script coverage now run through
the canonical `aes67` command.

## TUI and GUI follow-up

Do not create a second RTP, PTP, or CPAL implementation for an interface.
The next UI phase adds adapters over the same engine:

```text
apps/
├── aes67-cli/       # current command adapter
├── aes67-tui/       # terminal presentation
└── aes67-desktop/   # GUI host
            │
            ▼
       aes67-engine
```

Before implementing the GUI, evolve engine operations into typed commands,
snapshots, events, and cancellation handles. The CLI formats errors and writes
standard I/O; a TUI or GUI renders engine state. No UI surface binds RTP
sockets, manages PTP, decodes audio, or controls CPAL directly.

The existing Send Queue terminal UI is the first candidate to become an
`aes67-tui` adapter once its presentation code is separated from engine calls.

## Remaining cleanup

1. Add direct discovery-to-listen selection without a temporary SDP file.
2. Add TUI and GUI adapters only after the engine command/event boundary is
   stable.

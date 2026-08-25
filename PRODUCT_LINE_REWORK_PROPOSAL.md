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
└── aes67-desktop/   # Tauri GUI host
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

### Chosen desktop technology: Tauri

Use **Tauri** for the production desktop host. It gives the GUI a web frontend
for fast iteration while preserving Rust for the network, PTP, audio, and device
work. This is an application shell decision, not a rewrite of `aes67-engine`.

```text
React / TypeScript desktop frontend
             │  typed Tauri commands + engine events
             ▼
Tauri Rust host ─────────────── aes67-engine
                                      │
                         audio / network / ptp
```

The GUI must use a small, explicit bridge rather than exposing engine objects
to JavaScript:

| Bridge direction | Contract |
| --- | --- |
| GUI → engine | `validate_send`, `start_send`, `stop_send`, `discover`, `start_receive`, `stop_receive`, `list_output_devices` |
| engine → GUI | operation snapshots, discovery changes, readiness changes, warnings, terminal results |
| cancellation | opaque operation/session IDs owned by the Rust host |

The frontend may edit draft configuration and render state. It must never bind
sockets, call CPAL, run PTP, parse RTP, or make timing decisions.

`egui` remains a reasonable option for a future developer-only monitor or route
diagnostic, where an all-Rust immediate-mode view is useful. It is not the
recommended customer GUI: the required connected-node workflow, native file
selection, refined information hierarchy, and rapid visual iteration are a
better fit for Tauri's web frontend.

### Interaction model shared by GUI and TUI

Send and Receive remain the only top-level product modes. A presentation surface
does not become a third product line.

```text
Send:    Source ──► Network ──► Ready ──► Start sending
Receive: Discover ─► Stream  ──► Output ──► Start listening
```

The desktop GUI uses the selected sparse, dark connected-node design: amber
describes the route and green describes readiness. Its Send workspace is a
modular routing canvas: operators can create and move Source and Stream
modules, connect a source output to a stream input, delete a route, and fan one
source out to many streams. A Stream accepts one selected source at a time, so
adding a new source connection replaces its prior route rather than implicitly
mixing audio.

This canvas is the approved multi-session product direction. The desktop now
starts and stops file-backed routes as one batch. The existing CLI keeps its
single-session contract, while the process-level routing runtime owns shared
PTP, source decoding, fan-out, and per-stream RTP/SAP state. Live-input capture
and release-grade source/device editing are not enabled yet.

### Multi-session routing foundation

The multi-stream runtime is a process-level engine service, not a loop that
creates today's `Aes67Sender` repeatedly. This prevents multiple routes from
competing for PTP sockets and clock state.

```text
CLI / TUI / Tauri
        │ typed commands + snapshots/events
        ▼
  RoutingEngine (one process host)
   ├── PTP runtime, keyed by interface + PTP domain
   ├── source runtimes: decode once and maintain each source timeline
   └── stream runtimes: RTP packetizer + socket + SAP per output stream
                      ▲
        one source packet fans out to one or many stream runtimes
```

The first routing rule set is deliberately simple:

| Concept | Rule |
| --- | --- |
| Source | File or live input; owns one audio timeline and format validation. |
| Stream | One AES67 destination, RTP identity, SAP configuration, lifecycle, and default-enabled output gain stage. |
| Route | Connects exactly one Source to exactly one Stream. |
| Fan-out | A Source may feed many Streams; it is read once per packet, while each Stream applies its own gain. |
| Stream input | A Stream accepts one Source. Connecting another Source replaces the prior route. |
| Mixing | Out of scope. Multiple Sources never silently mix into a Stream. |

Stream gain defaults to unity (`0 dB`) and is constrained to `-120 dB` through
`0 dB`. Values below `-120 dB` are normalized to mute and displayed as
`-inf`. Gain belongs to the Stream, not the Source or Route, so routing one
Source to several Streams never couples their output levels.

The engine owns IDs, configuration validation, route changes, runtime state,
and cancellation. `SourceId`, `StreamId`, and `OperationId` must be opaque
typed identifiers rather than names or GUI node IDs. A frontend may preserve
its canvas positions and labels separately; it cannot treat those as engine
identity.

The routing bridge extends the base UI bridge with these typed operations:

| Direction | Operations / payload |
| --- | --- |
| UI → engine | `create_source`, `update_source`, `remove_source`, `create_stream`, `update_stream`, `remove_stream`, `assign_source`, `remove_route`, `start_all`, `stop_all`, `get_routing_snapshot`, `get_runtime_snapshot`, `get_stream_sdp` |
| engine → UI | atomic `RoutingSnapshot` revisions; source/stream readiness; PTP state; validation failures; per-stream lifecycle and warning events |
| lifecycle | `OperationId` identifies asynchronous start/stop work; a cancelled or failed operation always produces a final event |

#### Desktop IPC and realtime telemetry

The local desktop uses Tauri IPC rather than an internal HTTP server. Typed
commands handle control operations and snapshot reads. Low-rate lifecycle and
discovery changes may use events, while ordered or higher-rate telemetry uses a
Tauri channel created by an explicit subscription command.

The current runtime publishes low-rate operational snapshots with lifecycle,
PTP state, uptime, packets, bytes, packet/bit rate, late-packet count, and
stream-level peak/RMS. Raw audio samples and per-packet data never cross IPC.
Future 20–30 Hz per-channel meters use a dedicated channel and render outside
the revisioned routing graph so they do not cause full-canvas React renders.
The detailed contract is in `apps/aes67-desktop/IPC_PROTOCOL.md`.

`RoutingSnapshot` is the shared model for the TUI and GUI. It contains source
definitions, stream definitions, route assignments, PTP/network readiness, and
per-stream status. GUI canvas coordinates, zoom, selection, and unsaved edits
are presentation state; the engine only receives a validated configuration
mutation when the user commits it.

#### First executable slice

The first two increments are complete:

1. Add a pure, in-memory `RoutingModel` to `aes67-engine`. It creates IDs,
   validates references, supports source fan-out, replaces a Stream's selected
   input atomically, removes dependent routes, and emits revisioned snapshots.
   Unit tests must cover these rules without audio devices, RTP, PTP, or UI.
2. Add the process-level `RoutingRuntime`: one shared PTP client, one decoded
   packet per routed Source, per-stream gain/RTP/SAP stages, generated SDP, and
   low-rate operational statistics behind start/stop/snapshot commands.

This gives the TUI and Tauri GUI one real, testable state and runtime model.

The TUI uses the same sequence rather than duplicating every advanced setting
on one screen:

```text
aes67 tui

 Send  > Source       [ audio.wav                     ]
         Network      [ 239.69.83.1:5004              ]
         Ready        [ PTP locked · SAP on            ]
         [ Start sending ]

 Receive > Discover   [ Studio A Program                ]
           Output     [ Built-in Output                  ]
           [ Start listening ]

 Tab mode  ↑↓ focus  Enter edit/confirm  Space start/stop  ? help  q quit
```

Implement the TUI with Ratatui and its default Crossterm backend. It should own
terminal setup, key handling, focus, and rendering only; it invokes the same
engine commands and subscribes to the same snapshots/events as the GUI.

### Delivery sequence

1. Extract the typed command, snapshot, event, and session-handle boundary in
   `aes67-engine`, with unit tests that do not involve a UI.
2. Move the current queue experience behind the new TUI adapter and add the
   Send/Receive route screens.
3. Create `apps/aes67-desktop` as the Tauri host and reuse the validated
   connected-node interaction model.
4. Connect the GUI to real engine events, then add the discovery-to-listen
   selection path and persisted UI preferences.

### Implementation status

- Complete: a typed, revisioned `RoutingModel` in `aes67-engine`, including
  source fan-out and one selected source per stream.
- Complete: a Tauri 2 host with typed commands for source, stream, and route
  configuration.
- Complete: the React and TypeScript routing canvas consumes engine snapshots
  in Tauri, with an explicit browser-only preview fallback.
- Complete: file-backed multi-stream Start/Stop, shared PTP and source fan-out,
  per-stream gain/RTP/SAP, generated SDP, and operational runtime statistics.
- Complete: native audio-file import and replacement directly from Source
  modules.
- Next: configuration drawers, native device selection, live-input capture,
  engine events, and high-rate per-channel meter subscriptions.

## Remaining cleanup

1. Add direct discovery-to-listen selection without a temporary SDP file.
2. Add release-grade source/device editors and live-input capture.
3. Move high-rate per-channel meters from snapshot polling to Tauri channels.

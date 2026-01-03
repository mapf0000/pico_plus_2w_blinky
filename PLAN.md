# Rust Host-Agent Implementation Plan

This plan targets a Rust-based host agent that replaces the current .NET serial agent. It will run on a host machine, talk to the USB Army Knife device over the USB CDC serial interface, and provide the same features: agent status polling, remote command execution, VNC screen streaming, and debug log capture. The goal is wire-compatible behavior with improved performance, robustness, and maintainability. This document is intentionally self-contained so it can be copied into a separate repo without access to the original source.

## Goal
- Implement a Rust replacement for the .NET serial agent that speaks the same USB CDC TLV protocol and feature set (command execution, VNC proxy, debug logs, agent status).

## Scope and assumptions
- Scope is host-agent only; the device firmware and USB mode switching remain unchanged.
- macOS-first implementation (`--target aarch64-apple-darwin`) with a clear platform abstraction for later Windows/Linux support.
- Wire compatibility must match the TLV framing and command IDs below.
- Serial transport is USB CDC ACM at 115200/8N1.
  - The device only listens on serial when it is in USB Serial mode.
  - When the device is in USB NCM (network) mode, serial commands are not available.
  - Runtime USB mode switching on the device supports OFF/HID/Storage; NCM is boot-time only.

## Quality goals (modernization)
- Async, non-blocking I/O with bounded queues and backpressure.
- Zero-copy or low-copy TLV parsing with incremental resync on corrupted frames.
- Adaptive throughput control (chunk sizing + pacing) to fit serial bandwidth.
- Structured logging and metrics for troubleshooting on locked-down hosts.

## Security considerations (optional)
- `Execute` runs arbitrary OS commands. Consider a build-time or runtime gate (flag/allowlist) and clear logging so operators can control when it is enabled.

## Protocol constraints (self-contained)
- TLV framing: 1-byte tag + 4-byte length (little-endian u32) + payload bytes.
- Device receive buffer caps payloads to 2048 bytes; agent -> device frames must stay <= 2048 bytes.
- Known command IDs:
  - `1 Execute` (device -> agent): payload is UTF-8 command string.
  - `2 DebugMsg` (device -> host): payload is ASCII/UTF-8 log message.
  - `3 WSCONNECT` (device -> agent): initiate VNC proxy session.
  - `4 WSDATA` (device -> agent): raw VNC/WebSocket bytes toward agent.
  - `5 WSDISCONNECT` (device -> agent): end VNC session.
  - `6 WSDATARECV` (agent -> device): VNC/WebSocket bytes back to device.
  - `7 RequestAgentStatus` (device -> agent): no payload.
  - `8 AgentStatus` (agent -> device): payload is machine name bytes.
  - `9 ExecuteResult` (agent -> device): command output chunk; agent caps total to ~8KB.
  - `10 MicPcmData` (device -> agent): raw PCM audio bytes; agent appends to `mic.pcm`.
- Agent must be tolerant of malformed tags/lengths; prefer resync over hard exit.

## Required behavior (self-contained)
- **Agent status**: device periodically sends `RequestAgentStatus`; agent responds with `AgentStatus` containing machine name and treats that as “connected.”
- **Command execution**: on `Execute`, run `/bin/sh -lc <command>` on macOS, capture stdout+stderr, cap to 8KB, send in 2KB `ExecuteResult` TLVs with pacing. (Windows/Linux variants live behind a platform abstraction.)
- **VNC proxy**: device’s web UI tunnels VNC over `WSCONNECT/WSDATA/WSDISCONNECT`; agent must run a local VNC server and bridge stream I/O to TLV frames via `WSDATARECV`.
- **Debug logs**: device emits `DebugMsg` frames; agent prints/stores them.
- **Mic capture**: when `MicPcmData` frames arrive, append payloads to `mic.pcm`.
- **Delivery/automation context**: the agent binary is typically delivered on a USB mass-storage image and can be launched via HID/DuckyScript keystrokes (e.g., when the host is unlocked). The Rust host-agent should keep the same invocation style (`vid=`, `pid=`, optional `cwd=`) so existing scripts can launch it without changes.

## Delivery considerations
- Package a read-only, 8 MiB USB mass-storage image with per-OS subfolders (start with macOS; add Windows/Linux later).
- Launch via HID scripts that copy the correct binary locally and run it with `vid=`, `pid=`, optional `cwd=`.

## Proposed architecture
- `core::tlv`: incremental parser and writer for the 1+4+N format, length validation, resync logic, and tests.
- `transport::serial`: async serial read/write tasks, bounded outbound queue, reconnect loop.
- `transport::discovery`: macOS port discovery using IOKit/IORegistry (or serialport listing) with VID/PID matching, optional `--port` override. Keep a platform shim for Windows/Linux later.
- `runtime`: state machine (Disconnected -> Connecting -> Ready), event routing, pacing control.
- `handlers::exec`: `/bin/sh -lc <command>` on macOS, cap output to 8KB, chunk to 2KB, schedule delays.
- `handlers::vnc`: VNC server + stream bridge that maps VNC I/O to `WSDATA` / `WSDATARECV`.
- `handlers::mic`: append `MicPcmData` to `mic.pcm`, flushing in batches.
- `cli`: parse `vid=`, `pid=`, optional `cwd=`, `--raw`, `--debug`, `--port`.

## Implementation plan
1. **Project bootstrap**
   - Use the existing Rust binary crate under `tools/host-agent`.
   - Select async runtime (`tokio`) and crates: `tokio-serial`, `bytes`, `clap`, `thiserror`, `tracing`.
   - Add macOS discovery support (IOKit/IORegistry or serialport listing), and isolate platform-specific code for future Windows/Linux ports.
2. **TLV codec**
   - Implement incremental decode over a `BytesMut` buffer with resync on invalid tag/length.
   - Implement `write_frame` with length checks and optional chunking helper.
   - Add unit tests for endianness, empty payloads, max length, and resync behavior.
3. **Serial discovery and connection**
   - Implement VID/PID matching via macOS IOKit/IORegistry (fallback to serialport listing).
   - Add device-change notifications if available; otherwise use polling with backoff.
   - Retry loop with exponential backoff and cached last-known port.
   - Optional manual override: `--port /dev/tty.*` bypasses discovery.
4. **Command dispatcher**
   - Parse tags into an enum mirroring `HostCommand`.
   - Route frames to handlers via a registry; keep handler work off the read loop.
5. **Agent status + debug**
   - On `RequestAgentStatus` send `AgentStatus` with machine name (UTF-8 bytes).
   - On `DebugMsg` print to console/log.
   - Add `--raw` mode to print non-TLV data for debugging.
6. **Execute command flow**
   - On `Execute`, run `/bin/sh -lc <command>` on macOS, capture stdout+stderr.
   - Cap output to 8KB and send in 2KB `ExecuteResult` TLVs.
   - Implement pacing via token-bucket or adaptive delay based on write queue depth.
7. **VNC bridging (phase 2, deferred)**
   - Reference behavior (current repo): a no-auth VNC server runs locally, screen capture is platform-specific, and VNC I/O is tunneled over serial using `WSDATA` and `WSDATARECV`. Frames are JPEG-compressed at a low update rate to fit serial bandwidth.
   - Implement a `TransportStream` equivalent:
     - `WSDATA` feeds bytes into a read queue for the VNC server.
     - VNC output is chunked into `WSDATARECV` frames (2KB chunks).
   - Evaluate Rust crates for VNC server support and screen capture (prefer DXGI-backed).
8. **MicPcmData**
   - Append incoming payloads to `mic.pcm` in the working directory.
9. **CLI + packaging**
   - Accept `vid=`, `pid=`, and optional `cwd=` for parity with `Program.cs`.
   - Build instructions and usage examples in a README.
   - Provide a macOS build target (`--target aarch64-apple-darwin`) and a sample invocation like: `host-agent vid=cafe pid=403f`.
10. **Integration testing**
   - Unit tests: TLV encode/decode, chunking logic, max payload enforcement.
   - Property-based tests for TLV fuzz/resync.
   - Manual tests with hardware:
     - Agent status detection (`RequestAgentStatus` -> `AgentStatus`).
     - Execute command and verify `ExecuteResult` in web UI logs.
     - VNC connection via `/vnc/index.html`.

## Risks / open questions
- VNC server availability in Rust: may need a custom RFB layer if crates are incomplete.
- Device discovery: macOS IOKit/IORegistry availability vs serialport listing reliability.
- Throughput: serial bandwidth is low; pacing and backpressure need tuning to avoid timeouts.
- Delivery: packaging and launch method for the agent binary on the target host.

## Deliverables
- Rust host-agent binary with protocol parity.
- Minimal README + build steps.
- Basic test coverage for TLV framing and chunking.

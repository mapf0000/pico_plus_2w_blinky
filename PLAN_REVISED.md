# Revised Plan: Mac Host Agent + USB Device Protocol (VNC Deferred)

## Goals
- Build a Rust host daemon (mac target first) that speaks the existing TLV protocol over USB CDC.
- Add a robust device-side USB control channel to exchange TLV frames with the host daemon.
- Keep architecture modular so VNC streaming can be added later without refactoring core transport.

## Non-goals (for this revision)
- VNC server, screen capture, or stream bridging.
- Windows-specific discovery or command execution behavior (mac first).

## Current repo reality (relevant to planning)
- Firmware uses USB CDC for logging and HID for keyboard injection; no TLV control channel exists yet.
- HTTP/WS control is for USB enablement and HID scripts only.
- There is a log ring buffer (`src/log_buffer.rs`) that can be reused to emit `DebugMsg` TLVs.

## Protocol summary (device <-> host)
- TLV framing: 1 byte tag + 4 byte little-endian length + payload.
- Max payload length: 2048 bytes (agent must not exceed).
- Required tags:
  - 1 Execute (device -> host): UTF-8 command.
  - 2 DebugMsg (device -> host): log string.
  - 7 RequestAgentStatus (device -> host): no payload.
  - 8 AgentStatus (host -> device): machine name bytes.
  - 9 ExecuteResult (host -> device): 2 KB chunks, 8 KB total cap.
  - 10 MicPcmData (device -> host): raw PCM (optional if device supports mic).
- Error handling:
  - Drop frames with invalid length (> 2048) or malformed headers.
  - Resync by scanning for plausible tags and length.

## Architectural decisions for future-proof integration
- Shared protocol crate (`crates/agent-proto`):
  - `no_std` friendly, used by firmware and host.
  - Defines tags, max sizes, and TLV encode/decode helpers.
- Dedicated CDC-ACM interface for agent control:
  - Keep existing CDC logger separate to avoid mixing logs with TLV.
  - Assign distinct interface string(s) for future device discovery.
- Clear layering:
  - `transport` (serial) -> `codec` (TLV) -> `runtime` (state machine) -> `handlers`.
  - Makes it trivial to add a different transport (NCM, BLE, WS) later.
- Capability forward-compat:
  - Reserve tags for optional `Capabilities` or `Hello` message.
  - Only send if both sides opt-in (no behavior change to current protocol).

## Host daemon (mac) implementation plan
1. Create a new binary crate (proposed: `tools/agentd`).
2. Add dependencies:
   - `tokio`, `tokio-serial` (or `serialport` + `tokio::task::spawn_blocking`),
   - `bytes`, `clap`, `thiserror`, `tracing`, `tracing-subscriber`.
3. Implement TLV codec (in `crates/agent-proto`):
   - Incremental decode using `BytesMut`, with resync.
   - `write_frame(tag, payload)` with length checks and chunk helper.
4. Serial discovery for mac:
   - Default: `serialport::available_ports()` filter by VID/PID from CLI.
   - If multiple matching ports, prefer interface string or path pattern.
   - Always allow `--port` override for deterministic selection.
5. Runtime state machine:
   - `Disconnected -> Connecting -> Ready`.
   - Track last `RequestAgentStatus` timestamp; send `AgentStatus` immediately on connect and on request.
6. Handlers:
   - Execute: run `/bin/sh -lc <cmd>` with optional `cwd=`, capture stdout+stderr.
   - Cap output to 8 KB, send 2 KB `ExecuteResult` chunks with pacing.
   - DebugMsg: log to console and optional file.
   - MicPcmData: append to `mic.pcm`.
7. Backpressure:
   - Bounded outbound queue (e.g., 32 frames).
   - If queue depth is high, delay or drop low-priority frames.
8. Observability:
   - Structured logs with `tracing`.
   - Optional `--debug` to dump raw bytes and TLV stats.
9. Tests:
   - TLV codec unit tests + fuzz/resync tests (host side).
   - Integration tests using a loopback mock (no hardware).

## Device firmware (USB daemon) implementation plan
1. Add a new USB CDC-ACM class dedicated to the agent protocol:
   - Keep the existing CDC logger as-is.
   - Assign a stable interface string (ex: "agent") if supported by `embassy-usb`.
2. Implement `usb::agent` module:
   - Read/write tasks for the CDC class.
   - Use `agent-proto` codec for TLV parsing and framing.
3. Device-side "agent daemon" task:
   - Periodically send `RequestAgentStatus` (ex: every 2s).
   - Handle `AgentStatus` to mark host connected; expose state to UI.
   - Forward `ExecuteResult` to UI/logs (store last N lines in a ring).
4. Debug logs over TLV:
   - Periodically flush `log_buffer` entries as `DebugMsg` frames.
   - Rate limit to protect USB bandwidth.
5. Execute command source (device -> host):
   - Add a new WS command (ex: `HOST_EXEC <cmd>`) to send `Execute`.
   - Optional: display UI button to run a quick command for testing.
6. MicPcmData:
   - Stub the send path for now, but keep the tag reserved.
7. Resilience:
   - If host disconnects, stop sending requests and back off.
   - On reconnect, restart status polling.

## Required changes in this repo (pico_rust)
- Add `crates/agent-proto` (new) for shared TLV types and codec.
- Add `tools/agentd` (new) for the mac host daemon.
- Update `Cargo.toml` workspace members to include the new crates.
- Extend USB stack:
  - `src/usb/task.rs`: create second CDC class for agent control.
  - `src/usb/mod.rs`: export new agent module.
  - `src/usb/agent.rs` (new): CDC read/write + TLV handling.
- Add device-side agent task:
  - `src/agent.rs` or `src/usb/agent_daemon.rs` (new).
  - Hook into UI status or logging as needed.
- Extend WS API for host exec:
  - `src/http/routes/ws.rs`: add a command to send `Execute`.
  - Update frontend if you want a UI control now (optional for MVP).
- Docs:
  - `README.md`: add build/run instructions for `tools/agentd`.

## Milestones (no VNC)
1. Protocol crate + host daemon skeleton (codec, serial, CLI).
2. Device CDC control channel + status ping/pong.
3. Execute flow end-to-end (device command -> host exec -> device result).
4. Debug log forwarding over TLV.
5. MicPcmData stub (host file append only).

## Open questions to resolve early
- How to disambiguate dual CDC ports on mac (interface string vs manual `--port`)?
- What UI surface should trigger `Execute` on the device (web UI vs display)?
- Should `AgentStatus` include a small capability bitmap for future features?

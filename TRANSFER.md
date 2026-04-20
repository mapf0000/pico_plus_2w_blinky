# Browser Download Manager Plan (Host-Agent -> Device -> Browser)

## 1. Objective

Implement an end-to-end file transfer pipeline where the **host-agent is the sender** and the browser receives data through the device, with a rich in-browser download manager that shows:

- Transfer status: `open`, `downloading`, `finished`, `failed`, `aborted`
- Per-transfer progress: `% downloaded`, bytes received/total
- Per-transfer rate: smoothed bytes/sec
- Per-transfer ETA: estimated time remaining
- Chunk-level visibility: chunk count, each chunk status, per-chunk ETA, retries, errors

The implementation should be robust under packet fragmentation, reconnects, and backpressure, and should be developed test-first for protocol/state-machine-critical paths.

## 2. Scope and Constraints

- Direction: `host-agent -> USB CDC TLV -> firmware -> WebSocket -> browser`
- Existing transport:
  - Host-agent TLV framing (`apps/host-agent/src/tlv.rs`)
  - Firmware CDC control class (`firmware/src/usb/ctrl.rs`)
  - Firmware WS endpoint (`firmware/src/http/routes/ws.rs`)
  - Frontend single-request WS client (`apps/frontend/src/api.rs`) must be refactored for streaming/multiplexing
- Existing hard constraints:
  - TLV payload max currently `2048` bytes on host-agent side
  - Firmware control path currently has a very small receive accumulation buffer and must be redesigned for streaming safety
- Non-goals for v1:
  - Resume after browser reload
  - Multi-device transfer fan-out
  - Cryptographic confidentiality (integrity is in scope)

## 3. Architecture Overview

1. Host-agent reads file and sends framed chunk stream over CDC TLV.
2. Firmware acts as a streaming relay with minimal buffering and strict backpressure.
3. Browser receives metadata and chunk stream via WebSocket.
4. Browser persists chunks in IndexedDB (not localStorage), tracks live transfer/chunk state, computes rate + ETA, and exposes a download/finalization action.

## 4. Protocol Design

### 4.1 CDC TLV extension (host-agent <-> firmware)

Use new tags in an isolated range (example `20..=29`) to avoid collisions with existing tags:

- `TAG_FILE_OPEN`
- `TAG_FILE_CHUNK`
- `TAG_FILE_ACK`
- `TAG_FILE_CLOSE`
- `TAG_FILE_RESULT`
- `TAG_FILE_ABORT`
- `TAG_FILE_HEARTBEAT` (optional for long transfers)

### 4.2 Message schema

All payloads are little-endian encoded fixed header + variable tail.

- `FILE_OPEN`:
  - `protocol_version: u16`
  - `transfer_id: u64`
  - `total_size: u64`
  - `chunk_size: u16`
  - `chunk_count: u32`
  - `sha256: [u8; 32]`
  - `file_name_len: u16`
  - `file_name_utf8: [u8; file_name_len]`
- `FILE_CHUNK`:
  - `transfer_id: u64`
  - `chunk_index: u32`
  - `offset: u64`
  - `payload_len: u16`
  - `payload: [u8; payload_len]`
  - `chunk_crc32: u32`
- `FILE_ACK`:
  - `transfer_id: u64`
  - `highest_contiguous_chunk: u32`
  - `next_expected_offset: u64`
  - `window_credit: u16`
- `FILE_CLOSE`:
  - `transfer_id: u64`
  - `sent_chunk_count: u32`
  - `sent_total_size: u64`
- `FILE_RESULT`:
  - `transfer_id: u64`
  - `result_code: u8` (`ok`, `hash_mismatch`, `size_mismatch`, `aborted`, `internal_error`)
  - `detail_len: u16`
  - `detail_utf8`
- `FILE_ABORT`:
  - `transfer_id: u64`
  - `reason_code: u8`
  - `detail_len: u16`
  - `detail_utf8`

### 4.3 Protocol best practices

- Version every transfer (`protocol_version`) and reject unsupported versions clearly.
- Keep messages idempotent:
  - Duplicate `FILE_OPEN` with same `transfer_id` is accepted if metadata matches.
  - Duplicate `FILE_CHUNK` is ignored or re-ACKed.
- Always include explicit terminal events (`FILE_RESULT`/`FILE_ABORT`).
- Enforce strict bounds:
  - `payload_len <= negotiated_chunk_size`
  - `offset + payload_len <= total_size`
  - `chunk_index < chunk_count`
- Do not rely on serial line coding for reliability; rely on app-level ACK and retries.

## 5. Firmware Relay Design

### 5.1 Critical redesign in CDC receive path

Current receive accumulation in `firmware/src/usb/ctrl.rs` is too small for robust chunked transfer. Replace with a streaming TLV parser:

- Ring buffer or streaming decoder state machine
- No assumption that one packet == one frame
- Support partial header, partial payload, and multiple frames per packet
- Safe resync on invalid length/tag

### 5.2 Relay behavior

- Maintain per-transfer relay state keyed by `transfer_id`.
- Forward metadata and chunk events to WS subscribers.
- Backpressure rules:
  - Firmware only advertises `window_credit` it can safely relay.
  - If WS side stalls, reduce credit to 0 and pause host-agent sender.
- Timeout and cleanup:
  - Abort stale transfers after configurable inactivity timeout.
  - Free per-transfer resources deterministically.

## 6. WebSocket Protocol (firmware <-> browser)

Use multiplexed WS semantics:

- Text JSON for control/state events
- Binary WS frames for chunk payloads (preferred for efficiency)

Control events (JSON):

- `transfer/open`
- `transfer/progress`
- `transfer/chunk_status`
- `transfer/finished`
- `transfer/failed`
- `transfer/aborted`
- `transfer/ack_request` (if browser-managed ack windowing is enabled)

Binary chunk envelope:

- Fixed binary header:
  - `transfer_id: u64`
  - `chunk_index: u32`
  - `offset: u64`
  - `payload_len: u16`
  - `crc32: u32`
- Followed by raw chunk bytes

Best practices:

- Make control events self-describing and forward-compatible (`event_type`, `version`).
- Do not block command-response API while streaming; move from single pending response model to event dispatcher.

## 7. Frontend Download Manager Design

### 7.1 Storage strategy

- Use IndexedDB for chunk persistence and metadata.
- Avoid localStorage for binary or high-frequency updates.
- Keep in-memory cache small (active window only).

### 7.2 State model (idiomatic + testable)

Represent transfer/chunk lifecycle with enums:

- `TransferState`: `Open`, `Downloading`, `Verifying`, `Finished`, `Failed`, `Aborted`
- `ChunkState`: `Open`, `Downloading`, `Finished`, `Retrying`, `Failed`

Use a reducer-style state machine to process events deterministically.

Track per transfer:

- `transfer_id`
- `file_name`
- `total_size`
- `received_size`
- `chunk_count`
- `finished_chunks`
- `failed_chunks`
- `started_at`
- `updated_at`
- `smoothed_rate_bps`
- `eta_total`
- `status`

Track per chunk:

- `chunk_index`
- `size`
- `received`
- `attempts`
- `started_at`
- `finished_at`
- `status`
- `instant_rate_bps`
- `eta_chunk`

### 7.3 Rate and ETA calculations

Use monotonic timestamps and EWMA smoothing:

- `instant_rate = delta_bytes / delta_time`
- `smoothed_rate = alpha * instant_rate + (1 - alpha) * previous_smoothed_rate` (`alpha ~= 0.2`)
- `eta_total = (total_size - received_size) / max(smoothed_rate, epsilon)`
- `eta_chunk = (chunk_size - chunk_received) / max(chunk_rate, epsilon)`

Rules:

- Clamp and saturate to avoid negative or overflow values.
- Show `ETA unknown` when insufficient samples.
- Update UI cadence at 200-500 ms to avoid render thrash.

### 7.4 UX behavior

- Main table: one row per transfer with status, progress bar, rate, ETA, controls.
- Chunk detail panel per transfer:
  - Chunk status counts
  - Current downloading chunk(s)
  - Retry/error list
- For large chunk counts, virtualize list rendering and show aggregated counters by default.

## 8. Idiomatic Rust Plan

### 8.1 Type modeling

Use strong domain types:

- `TransferId(u64)`
- `ChunkIndex(u32)`
- `ByteCount(u64)`
- `BytesPerSec(u64)`
- `ProtocolVersion(u16)`

Benefits:

- Compile-time separation of semantically different integers
- Cleaner APIs and less accidental field mix-up

### 8.2 Error handling and boundaries

- `thiserror` for layered error enums (`CodecError`, `ProtocolError`, `RelayError`, `StorageError`)
- `Result` across boundaries; avoid panics in transfer path
- `#[non_exhaustive]` on public protocol enums likely to evolve

### 8.3 Concurrency and backpressure

- Bounded `tokio::sync::mpsc` channels between parser/relay/sender
- `tokio::select!` for cancellation, timeout, and channel coordination
- Avoid unbounded queues in hot path

### 8.4 Serialization

- Binary payloads encoded manually or via zero-copy-friendly helpers
- Control JSON with strict serde contracts (`deny_unknown_fields` where appropriate)

## 9. TDD Strategy (Neuralgic Points First)

### 9.1 Neuralgic points

1. TLV fragmentation/reassembly correctness
2. ACK window, retries, and duplicate chunk handling
3. Transfer state-machine transitions and terminal states
4. Rate/ETA math stability with jittery timestamps
5. Firmware relay backpressure and memory safety
6. Frontend WS multiplexing (stream + command responses)

### 9.2 Test-first sequence

1. **Codec tests first**
   - Round-trip encode/decode for all message types
   - Fragmented input fuzz tests
   - Invalid length/tag resync tests
2. **State-machine tests**
   - Given event streams, assert exact state transitions
   - Duplicate `FILE_CHUNK` and duplicate `FILE_OPEN` semantics
   - Abort and timeout transitions
3. **Rate/ETA deterministic tests**
   - Inject fake clock
   - Validate EWMA convergence and edge cases (`0 bytes`, pauses, spikes)
4. **Backpressure tests**
   - Simulated slow WS receiver
   - Assert host-agent sender pauses/resumes via ACK credit
5. **Integration tests (PTY)**
   - Extend `apps/host-agent/tests/e2e_mac.rs` with transfer scenarios
   - Simulate frame fragmentation and reconnect behavior
6. **Frontend wasm tests**
   - Reducer tests for chunk/transfer status updates
   - UI formatter tests for `%`, rate strings, ETA labels
7. **End-to-end smoke**
   - Transfer small, medium, large files
   - Forced abort, hash mismatch, induced timeout

## 10. Implementation Phases

### Phase 0: Design lock

- Finalize tag IDs and schemas.
- Finalize transfer state machine and ACK policy.
- Add protocol markdown + binary examples.

Acceptance:

- Protocol doc complete with examples and failure semantics.

### Phase 1: Host-agent protocol + sender core

Files:

- `apps/host-agent/src/tlv.rs` (message support)
- `apps/host-agent/src/dispatch.rs` (new file transfer handlers)
- New modules:
  - `apps/host-agent/src/transfer/mod.rs`
  - `apps/host-agent/src/transfer/codec.rs`
  - `apps/host-agent/src/transfer/sender.rs`
  - `apps/host-agent/src/transfer/state.rs`

Acceptance:

- Host-agent can stream file chunks with retry + ACK handling in test harness.

### Phase 2: Firmware CDC parser + relay

Files:

- `firmware/src/usb/ctrl.rs` (streaming parser redesign)
- New module: `firmware/src/usb/file_transfer.rs`
- `firmware/src/http/routes/ws.rs` (control + binary forwarding)

Acceptance:

- Firmware relays transfer events and chunk data without overrun under fragmented input.

### Phase 3: Frontend WS refactor + storage

Files:

- `apps/frontend/src/api.rs` (multiplexed WS client)
- New modules:
  - `apps/frontend/src/transfer/store.rs`
  - `apps/frontend/src/transfer/reducer.rs`
  - `apps/frontend/src/transfer/indexed_db.rs`
  - `apps/frontend/src/transfer/metrics.rs`

Acceptance:

- Browser stores incoming chunks in IndexedDB and maintains accurate transfer model.

### Phase 4: Download manager UI

Files:

- `apps/frontend/src/lib.rs` (new transfer manager section)
- `apps/frontend/ui/style.css` (status/graph/table styles)

Acceptance:

- UI shows required statuses, chunk counts, `%`, rates, ETA (chunk + total), and completion.

### Phase 5: Hardening and tuning

- Throughput tuning (chunk size/window)
- Better diagnostics and telemetry
- Limits and guardrails

Acceptance:

- Sustained transfer stability with no memory growth and predictable recovery on fault injection.

## 11. Performance and Reliability Guidelines

- Start conservative defaults:
  - `chunk_size = 1024`
  - `window_credit = 4`
  - Retry timeout based on smoothed RTT with clamp
- Adaptive tuning:
  - Increase credit/chunk size after stable ACKs
  - Decrease aggressively on timeout (`AIMD`-style)
- Keep transfer logs structured (`transfer_id`, chunk range, retries, RTT, credits).

## 12. Security and Safety

- Validate all lengths and indexes before allocation.
- Enforce max transfer size and max concurrent transfers.
- Verify final SHA-256 before marking finished.
- Sanitize file metadata for UI display (escape/length clamp).
- Treat all incoming protocol data as untrusted.

## 13. Observability and Diagnostics

- Structured logs in host-agent and firmware for each protocol edge.
- Frontend debug panel:
  - Active transfer IDs
  - Current credits/chunk window
  - Smoothed rate and RTT
  - Last error reason
- Add counters:
  - Chunks sent/acked/retried/failed
  - Bytes relayed
  - Transfer completion ratio

## 14. Done Criteria

Feature is done when:

- End-to-end transfer works reliably from host-agent to browser.
- Download manager displays:
  - overall status (`open`, `downloading`, `finished`, `%`, ETA)
  - chunk count and per-chunk status (`open`, `downloading`, `finished`, retry/fail)
  - live transfer rate
- TDD coverage exists for all neuralgic protocol and state-machine paths.
- Fault scenarios (timeout, duplicates, abort, corruption) are deterministic and user-visible.

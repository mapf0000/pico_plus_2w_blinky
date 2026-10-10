# Native companion: remaining feature-parity plan

Updated 2026-10-10. This is the active backlog for replacing the former Web UI
with the native Rust/egui Bluetooth companion. Completed implementation and test
history belong in [DEVICE_TESTING.md](docs/DEVICE_TESTING.md); remove completed
items from this backlog as milestones land.

## Baseline and scope

Bluetooth discovery, connection, provisioned-secret authentication, control acquisition,
USB on/off, layout-aware text effects and cancellation are implemented. The user
confirmed the previous v2 pairing, real USB keyboard output and delayed cancellation
on hardware. BLE v3 now has host/embedded validation plus macOS/Pico acceptance
for unattended control/status/reconnect, wrong-key rejection and unauthenticated
command/continuous-read timeout rejection. V3 HID acceptance remains separate.
WLAN, HTTP/WebSocket, the Yew frontend and browser Python Worker have been removed.
The target computer still connects to the Pico through USB HID/CDC and runs the
host agent. The control computer runs the companion and connects through BLE.

Keep egui/Glow, the bounded backend, one BLE connection and one HID job. Preserve
connection ownership, priority cancellation and independent local payload execution.
The user requires operation without physical Pico access, including the first
connection. Replace numeric comparison with provisioned-secret authentication as
milestone 0. BLE v3 now implements this without a pairing gate or display overlay.
Add concrete operations as needed; no WLAN fallback or general transport framework
is required. Python runs on the control computer.

Parity means the former UI's useful workflows, including failure handling and
security boundaries. Packaging and platform acceptance are additional release
work. Shell execution and credential prompts are optional extensions: they were
on-device daemon operations, not controls in the former Web UI. USB mass storage
was already removed before this baseline and is not a parity requirement.

## Research baseline and gaps

Compared current source with commit `194e7c2`, the last commit before the
Bluetooth-only removal (`2f6579c`). Historical sources can be read with
`git show 194e7c2:<path>`; the old paths below are intentionally absent today.

| Former workflow | Current companion / reusable code | Remaining work |
| --- | --- | --- |
| Unattended authorization (new deployment requirement) | BLE v3 Noise PSK, private provisioning profile, encrypted control/snapshots and no physical gate implemented | Board acceptance, OS credential-store import/profile selection and authenticated atomic rotation |
| Overview: USB state, firmware version/build, host OS, agent presence/version, CDC installation stage, compatibility errors | Basic build/protocol/USB/presence/uptime/RTT; `firmware/src/capabilities.rs` and `usb/bootstrap.rs` retain richer state | Versioned capability/metadata response, freshness and agent compatibility, bootstrap progress and actionable errors |
| Start/stop USB | Implemented; fault cases remain unverified | Hardware acceptance for enable/disable, detach and job contention |
| USB manufacturer/product editing and persistent save | `device_config.rs` still reads the two existing flash slots; mutation/save code was removed | Restore validated, transactional saving while USB is off; native editor and readback |
| Python editor, generator execution, Start/Stop and errors | Text demo only; keyboard lowering, script envelopes and strict HID execution remain | Supervised interpreter, editor, generator send/throw, deadlines, local waits and capability checks |
| Python `wait_event()` for buttons and connection/USB/agent changes | Status can supply capability transitions; device button publisher was removed | Authenticated, bounded button events; supervisor event routing and lifecycle semantics |
| Host filesystem navigation | Agent listing/pagination and firmware page validation remain | BLE request/page relay; Home/Root/Up, breadcrumbs, hidden files, metadata, selection, pagination and errors |
| Encrypted path/default-path transfer initiation and session reconnect | Agent session/crypto/sender and shared `transfer-crypto` / `transfer-protocol` remain | Native session client, authenticated BLE relay and transfer controls |
| Downloads: queue, progress, rate/ETA, verification, save | No native receiver; `usb/events.rs` deliberately rejects delivery | Bounded stream-to-disk receiver, verification/receipt, safe saving and transfer activity UI |
| Diagnostics: connection activity, filtering, errors-only, clear and autoscroll | Bounded static-message diagnostics | Bounded structured entries, operation/error context and equivalent viewing controls |
| Connection loss and reconnect | Explicit reconnect and incarnation checks exist | Convenient recovery with fresh secret-authenticated sessions, stale-response rejection and no execution replay; platform acceptance |

Historical evidence: `apps/frontend/src/app.rs`, `ui/overview.rs`, `ui/scripts.rs`,
`ui/transfers.rs`, `ui/diagnostics.rs`, `api.rs`, `python.rs`,
`apps/python-worker/src/lib.rs`, `docs/SCRIPTING.md`, and
`firmware/src/http/routes/ws.rs` at `194e7c2`.

Two distinctions prevent unnecessary work:

- The old USB Mac/Windows selector wrote `host.rs` state for status reporting;
  no USB execution path consumed it. Display the agent-reported OS and keep the
  existing target keyboard layout selector. Do not restore a nonfunctional toggle.
- Execute/credential capabilities in old HELLO did not create browser RPCs.
  Reaching Web UI parity does not require exposing either over BLE.

## 0. Unattended authentication: acceptance and key lifecycle

Implemented: allocation-free `ble-session` using fixed
`Noise_NNpsk0_25519_ChaChaPoly_SHA256`, checked X25519, directional authenticated
records, BLE v3 negotiation, bounded MTU-23 fragmentation and encrypted snapshots.
Empty handshake payloads cannot execute effects. Connect with a matching profile
authenticates; encrypted Acquire grants control. Commands/results stay encrypted.
SMP pairing and the numeric-comparison display/button gate are removed. Physical
Y Stop and incarnation-scoped cancellation remain.

The companion generates/imports private per-device profiles and matching factory
images. Firmware reads a versioned CRC-checked two-slot reservation, separate from
USB identity and absent from normal ELF load segments. No default key or USB/BLE
key export exists. Snow interoperability, wrong key/context, first-handshake
replay, record replay/tampering/reflection, sequence gaps, fresh sessions, maximum
records, response framing and provisioning corruption have software coverage.
See [the wire specification](docs/PROTOCOL.md#bluetooth-control-v3),
[provisioning commands](apps/companion/README.md#trusted-provisioning) and
[acceptance evidence](docs/DEVICE_TESTING.md).

Remaining:

- Record independent power-cycle authentication, missing-key device behavior,
  forced default ATT MTU, interrupted maximum upload/response, Cancel latency,
  v3 HID/key release and local-preset contention. Unattended acquire/release,
  encrypted status/reconnect, wrong key, unauthenticated command rejection, timeout
  under continuous reads and key persistence across ELF deployment have passed
  on macOS/Pico. Measure standalone handshake time and stack headroom;
  compilation/linked static size does not establish runtime stack usage or entropy.
- Import/select profiles into each supported OS credential store and reconnect
  without repeatedly supplying a file. Current `--profile PATH` is a private-file
  prototype; macOS/Linux permissions are checked, Windows ACL handling is not
  accepted. Keep explicit trusted provisioning export and secure external backups.
  Do not place production credentials only under `target/`.
- Implement authenticated atomic key rotation after agreeing its recovery behavior.
  Proposed flow: generate and securely persist the new profile on the trusted PC
  first; rotate only over an authenticated acquired idle connection; write/verify
  the inactive flash slot and make the CRC-valid higher sequence the commit point.
  Keep the current old-key session only long enough to acknowledge, then disconnect;
  new connections accept only the committed key. If acknowledgement is lost, the
  PC retains both candidates and determines the committed one through fresh
  authentication, never an unauthenticated query/reset. Validate flash-write safety
  alongside BLE, interruption at every write phase and reboot before enabling this.
  Rotation is not yet callable, and the two reserved slots alone are not evidence
  of working transactional rotation.

Possession of the per-device key grants authority rather than identifying a
particular computer. Losing every backup needs another trusted reprovisioning
route. The hostile USB target/agent never receives this key. Firmware flash is not
encrypted at rest; physical extraction/firmware compromise is outside the boundary.
The prototype bounds unauthenticated occupancy and disconnect rate, not radio DoS.

## 1. Stable controls, complete overview and USB identity

Implement in small changes: polling/UI state first, metadata next, identity last.
Use milestone 0's protected record path for metadata/configuration requests and
responses; milestone 2 extends it with streamed pages/events and receiver credit.
The flicker fix can land independently of authentication.

- **Button flicker:** the user sees flicker roughly every second. Code inspection
  shows `Client::tick()` creates a global pending Status request and buttons gate
  on `snapshot.pending`; this is a likely cause, not a confirmed rendering diagnosis.
  Separate background refresh from user-operation busy state. Queue a bounded
  user intent behind an in-flight read without allowing conflicting mutations;
  retain immediate Cancel/Disconnect priority. Verify appearance and clicks in
  the actual connected window over multiple polling cycles.
- Expose firmware version/build, supported layouts/limits, agent version/OS and
  presence freshness, filesystem/transfer versions, and CDC bootstrap stage/error.
  Reuse `capabilities.rs` and `usb/bootstrap.rs`; avoid parallel copies of state.
  Gate workflows by advertised support and agent availability. Keep sensitive
  metadata behind authenticated control; existing public discovery stays minimal.
- Restore configuration mutation and persistence using the current printable
  ASCII bounds (manufacturer 32 bytes, product 48). Reject edits while USB is
  active, report flash failures, and acknowledge success only after persistence.
  Preserve the existing slot/schema/layout and apply identity on next USB start.
  Review flash-write safety while the BLE stack is running.
- Add operation context, filtering, errors-only, clear and autoscroll to bounded
  diagnostics. Host data remains text; omit secrets, script text and file contents.

Acceptance: refresh never flashes controls or swallows clicks; unsupported or
absent-agent features explain why they are unavailable; identity validation and
flash failure are tested; saved identity survives reboot and USB enumeration.
Verify USB toggles, authentication failure/timeout, physical Y Stop, detach, disconnect,
partial-upload cancellation, near-limit effects and local-preset contention.
Record pending and completed board checks in DEVICE_TESTING.md.

## 2. Bounded BLE responses/events and filesystem browsing

This is the shared prerequisite for filesystem pages, Python button events and
file streaming. Today's finite 48-byte encrypted response carries control/status snapshots;
it cannot stream directory pages or file records. `usb/events.rs` is an unavailable
sink, so a new screen alone cannot restore these workflows.

- Extend `ble-protocol` with explicitly versioned request/response/event framing,
  correlation, connection ownership and capability negotiation, building on the
  protected metadata/configuration slice. Negotiate additions to BLE v3 without
  accepting a downgrade to the old control gate or plaintext operations.
- Add an authenticated outgoing GATT characteristic and native subscription.
  Use notifications with explicit application credit/acknowledgement for streaming;
  notification submission does not prove receiver consumption. Keep command
  results, Cancel/Disconnect and physical Stop responsive during streaming.
- Start with MTU-23 support. Measure negotiated MTU and test larger fragments
  within stack/characteristic limits. Current packets are eight fixed 128-byte
  buffers; a larger MTU requires an explicit packet-pool/memory review. Never
  assume an OS supplies a large MTU or increase all buffers indiscriminately.
- Replace the unavailable sink with one connection-owned bounded relay; separate
  pure payload validation from BLE delivery. Reuse the existing sole CDC owner,
  `CtrlCommand` queue and `usb/ctrl/relay` validation. Remove browser JSON/kind
  wrappers at this adapter boundary where practical; preserve USB wire versions.
- Define bounds before coding: fragment size, logical record maximum (USB payloads
  are at most 2,048 bytes), reassembly storage, queues, credits and deadlines.
  Reset partial uploads, pages and queued events on connection loss. No stale
  receiver can consume or acknowledge a replacement connection's records.
- Implement correlated filesystem list/cancel and validated pages using existing
  filesystem v1. Recover pure decoding/state from the old `filesystem.rs` into
  native code; replace Yew rendering. Preserve cursor/hidden/entry-type semantics,
  superseded-request cancellation and list deadlines; bound accumulated entries.
- Restore best-effort released-button events from display routing, excluding
  gestures consumed by menus, Payloads and device-wide Stop. Keep display
  hardware adapters out of protocol/state code.

Acceptance: fragmented maximum-size pages, malformed frames, queue saturation,
stale pages and reconnect are covered. Real browsing supports Home/Root/Up,
breadcrumbs, hidden files, Load more, path selection and actionable errors.
Run a synthetic record stream at MTU 23 and the negotiated larger MTU; record
throughput, queue high-water marks and Cancel/Stop latency. Set measured transfer
timeouts before building the download workflow. No assumed Wi-Fi-equivalent rate.

## 3. Native Python scripting

Depends on milestone 2 for complete event parity; interpreter/supervisor work can
start independently. Preserve the former Python API instead of inventing a DSL.

- Recover the pure VM/prelude and tests from the old Worker. Keep RustPython 0.6.0
  and its compatible Ruff 0.16.5 pins initially; avoid unrelated upgrades.
  Add a supervised child process launched only on Start, potentially a helper
  mode of the same executable, with bounded versioned IPC and hard termination.
  No interpreter execution in egui callbacks or the BLE actor.
- First run a small feasibility spike: generators, send/throw, infinite loops,
  allocation exhaustion, Stop, denied imports and startup/size/peak-RAM measurement.
  A child process provides termination/crash containment, not a filesystem/network
  sandbox. Choose and document the isolation/resource boundary before shipping.
  If native restrictions cannot preserve the former boundary simply, evaluate an
  interpreter-only WASM module with a native runtime and no host/WASI capabilities.
  This would not restore the browser frontend. Measure cost rather than assuming
  either interpreter packaging option is the lightest.
- Restore `layout`, `tap`, `modtap`, `text`, `sleep`, `wait_event`, `require_usb`
  and `require_host_agent`, the existing `PicoError` subclasses and generator
  send/throw behavior. Add a general validated keyboard-effect submission API to
  `companion-core`; today's UI action only submits text. Reuse script v1 and KBD1.
- Restore 32-KiB/1,024-line source limits, 30-second startup, 500-ms subsequent
  steps, 1,024-character text and 4,096-byte KBD1, one process/effect and bounded
  diagnostics. Establish an enforceable native memory limit; the former WASM
  linear-memory ceiling was 128 MiB. Maintain device-effect completion deadlines.
- Keep timers/event waits in the supervisor. Buffer at most 32 button events,
  dropping oldest; do not replay capability transitions or events lost offline.
  Connection loss throws into an outstanding effect without retrying keystrokes;
  a script may catch the error and retain state while the app remains open.
  App exit/Stop kills the child and best-effort cancels its scoped device effect.
- Add source editor, Start/Stop, Ctrl/Cmd+Enter, explicit running/waiting/fault
  states, bounded diagnostics and API help. Source edits affect the next run.

Acceptance: former API examples and generator tests pass, including loops,
send/throw and exception recovery. A non-yielding script cannot freeze the UI;
Stop and timeouts terminate it. Verify button waits, capability changes,
disconnect/re-authentication, HID cancellation and key release on hardware.

## 4. Encrypted file reception and saving

Depends on milestone 2. Reuse transfer/session v2 and the current host agent;
port pure session/store logic from the old `transfer/secure.rs` and `store.rs`.
Replace browser IndexedDB/download adapters with native streaming storage.

- Establish the existing in-memory Noise session with the host agent over the
  secret-authenticated, acquired BLE connection. Retain per-file AEAD, strict lengths,
  ordering, SHA-256 and the encrypted final receipt; no plaintext downgrade.
  The control PSK authenticates the companion/Pico session, not the source computer
  or file. Keep it separate from the host file-transfer session and keys.
- Carry session envelopes and encrypted open/chunk/close records through the
  bounded relay. Retain firmware's public structure/order checks without giving
  it file keys. USB ACK credit must reflect bounded downstream capacity; BLE
  receiver credit advances only as authenticated records are accepted into bounded
  storage work. Never ACK dropped records or equate notifications with disk writes.
  Final success requires verification and the native receiver's encrypted receipt.
- Stream to app-owned temporary files on a worker, update the hash incrementally,
  and bound staging work, active transfers, history and cumulative storage.
  Handle full disk, write failures, app exit and disconnect with abort/cleanup.
  Never allocate a whole file in memory or restore the PSRAM WebSocket batch pool.
- Restore encrypted path input, Queue transfer, Set as default, Queue default and
  session reconnect. Show queued/receiving/verifying/finished/failed/aborted state,
  byte progress, rate/ETA and useful error/retry counts. Retry transport records
  only under the existing exact-ciphertext rules; never reuse a nonce with new data.
- Save verified files through an explicit destination choice. Treat host filenames
  as display metadata, not local paths: prevent traversal, overwrite surprises and
  symlink escapes. Never automatically open, execute or interpret downloaded files.

Acceptance: byte-exact multi-chunk and empty-file reception; authenticated manifest,
ordering/length/hash failures; malformed/duplicate records, retry/timeout, slow/full
disk, disconnect and saturated queues. Demonstrate a representative real file and
record throughput and control latency. Revise credits/timeouts from measurements,
including the existing five-second USB ACK timeout. Four firmware relay states do
not automatically justify four simultaneous native downloads.

## 5. Recovery, packaging and release acceptance

- Complete in-app profile import/selection and convenient reconnect, building on
  the implemented CLI profile workflow and actionable wrong-key error. Reconnect
  uses a fresh session without Pico interaction; preserve intentional control
  acquisition and never replay effects. OS bonding is not a prerequisite.
- Test repeated connect/control/cancel/disconnect cycles, soak, app shutdown and
  device reboot. Measure idle CPU/RAM, startup and release artifact size, including
  interpreter and staging components; keep heavyweight work lazy.
- Package the macOS app with the required Bluetooth permission metadata and helper
  discovery. Then validate real Windows/Linux BLE and their installation/permission
  paths; compilation alone is not platform acceptance. Do not add an updater first.
- Update README/operator workflows, AGENTS, architecture, protocol, scripting,
  threat model and device acceptance records with each implemented milestone.
  Legacy WebSocket sections should remain clearly historical until migrated.

Parity is reached when every workflow in the gap table works through the native
app with the USB target, has the stated failure handling, and has recorded hardware
acceptance. Packaging/platform claims require their own actual-device evidence.

## Technical sources and validation

Research used the repository snapshot above and installed dependency sources.
The [Noise specification](https://noiseprotocol.org/noise.html#pre-shared-symmetric-keys)
defines PSK handshakes; the application still owns framing, replay policy and
authorization timing. The [Noise-Rust project](https://github.com/blckngm/noise-rust)
documents allocation-free `no_std` support. Its RustCrypto defaults include system
RNG features: select/adapt primitives explicitly for the Pico rather than copying
the default dependency configuration. [Snow's documentation](https://docs.rs/snow/0.10.0/snow/)
states that its `no_std` mode needs allocation. The allocation-free implementation now uses Noise-Rust with a narrow X25519
adapter. Host interoperability and compilation do not establish hardware performance
or a complete security audit.

The installed btleplug 0.13.4 exposes `mtu()`, `subscribe()` and `notifications()`;
TrouBLE 0.6.0 provides the GATT stack. These APIs enable the proposed outgoing
path but do not establish throughput or application backpressure. See the
[btleplug Peripheral API](https://docs.rs/btleplug/latest/btleplug/api/trait.Peripheral.html).

The old Worker already has native RustPython tests and uses
`Interpreter::without_stdlib`; reuse is feasible, while native isolation remains
to be proven. For the optional WASM spike, Wasmtime documents
[sandbox boundaries](https://docs.wasmtime.dev/security.html) and
[execution interruption](https://docs.wasmtime.dev/examples-interrupting-wasm.html).
Its [configuration API](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html)
also explains why interruption needs bounded memory. No Python runtime dependency has
been selected or added.

Use the affected suites in [AGENTS.md](AGENTS.md#validation-matrix). Extend protocol
tests for malformed/boundary/order/ownership cases, supervisor tests for process
termination and reconnect, and receiver tests for storage/backpressure. Firmware
protocol/capacity/flash changes require an embedded release build and size review.
UI changes require the actual native window; authentication, HID and streaming require
board checks. Flash only when explicitly authorized. Update this backlog after
each accepted milestone; retain test evidence in DEVICE_TESTING.md.

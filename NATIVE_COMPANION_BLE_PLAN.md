# Native companion: remaining feature-parity plan

Updated 2026-10-11. This is the active backlog for replacing the former Web UI
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
Background reads no longer mark controls busy; one bounded command can wait
behind them, with queued-text cancellation and fail-closed timeout/disconnect.
Timestamped/contextual diagnostics now have filtering, errors-only, clear and
autoscroll. An acquired, encrypted overview reports the full build, execution
limits, agent metadata/freshness and CDC stage. Pico readback and commands during
refresh have passed. The user confirmed steady buttons across repeated refreshes
while connected with control acquired. Read-only host filesystem browsing and
released-button notifications now use an encrypted, credit-controlled BLE stream;
macOS/Pico pagination, cancellation and reconnect have passed. CDC installer-stage
and stale-agent acceptance remain pending.
USB identity editing, verified legacy-slot saving while USB is off, encrypted
readback, USB descriptor application, repeated stop/start and reconnect have passed.
See DEVICE_TESTING.md for completed slices.
WLAN, HTTP/WebSocket, the Yew frontend and browser Python Worker have been removed.
The target computer still connects to the Pico through USB HID/CDC and runs the
host agent. The control computer runs the companion and connects through BLE.

Keep egui/Glow, the bounded backend, one BLE connection and one HID job. Preserve
connection ownership, priority cancellation and independent local payload execution.
The user requires operation without physical Pico access, including the first
connection. BLE v3 implements provisioned-secret authentication without a pairing gate or
display overlay; milestone 0 tracks remaining acceptance and key lifecycle work.
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
| Overview: USB state, firmware version/build, host OS, agent presence/version, CDC installation stage, compatibility errors | Protected overview v1, full build/limits, version/OS/hostname/heartbeat age and CDC stage reuse existing firmware state | Populated/stale-agent and CDC-stage acceptance; installation byte progress/specific failure reasons if needed; gate future workflows by actual BLE support |
| Start/stop USB | Repeated BLE stop/start, live macOS removal and descriptor rebuild passed | Timeout/detach and job-contention fault acceptance |
| USB manufacturer/product editing and persistent save | Native editor, encrypted acquired read/save, verified inactive-slot transaction and USB-off lifecycle gate implemented; macOS descriptor/readback/reconnect checks passed | Independent power-cycle persistence and editor failure/draft acceptance; worst-case flash blackout measurement |
| Python editor, generator execution, Start/Stop and errors | Text demo only; keyboard lowering, script envelopes and strict HID execution remain | Supervised interpreter, editor, generator send/throw, deadlines, local waits and capability checks |
| Python `wait_event()` for buttons and connection/USB/agent changes | Encrypted released-button stream, 32-event native history and sampled status exist | Physical routing/Stop acceptance; supervisor event routing and lifecycle semantics |
| Host filesystem navigation | Encrypted BLE list/cancel/page relay; bounded native Home/Root/Up, breadcrumbs, hidden files, metadata, selection and pagination implemented; real agent pagination/error/cancel/reconnect passed | Interactive navigation acceptance, slow-agent/soak and Windows/Linux board checks |
| Encrypted path/default-path transfer initiation and session reconnect | Agent session/crypto/sender and shared `transfer-crypto` / `transfer-protocol` remain | Native session client, authenticated BLE relay and transfer controls |
| Downloads: queue, progress, rate/ETA, verification, save | No native receiver; `usb/events.rs` deliberately rejects delivery | Bounded stream-to-disk receiver, verification/receipt, safe saving and transfer activity UI |
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

## 1. USB identity and overview acceptance

The polling/diagnostics/finite-overview slice is implemented and flashed. It uses
the existing protected records, with a negotiated overview-v1 Info flag, a
288-byte plaintext bound and a 304-byte finite response slot. Ordinary snapshots
remain 48-byte ciphertext. No streaming relay or USB format changed. Detailed
metadata requires acquired control; layouts are companion lowering choices,
because firmware executes layout-independent KBD1. Completed checks belong in
DEVICE_TESTING.md; USB identity v1 now shares those records and the existing response slot.
Implementation is complete; the remaining acceptance below is still open.

Remaining acceptance (implementation and completed checks are in DEVICE_TESTING.md):

- Confirm saved identity after an independent power cycle, and exercise the native
  editor failure/draft behavior interactively (invalid input, rejected save, Reload,
  and edits across refresh). The user confirmed a successful name change; live
  macOS detach acceptance now checks actual registry removal as well as BLE flags.
  Flash failure/power loss is covered in host fault-injection tests; board save,
  readback, repeated USB toggles, descriptors and fresh reconnect have passed.
  Measure worst-case flash/radio blackout if field conditions require a tighter bound.
- Exercise diagnostic filtering, clear and manual scrolling with a populated view.
- Accept populated/stale/legacy agent metadata and CDC installer stages on hardware.
  Heartbeat age is sampled at overview time; live presence still expires at
  25 seconds. Decide whether byte progress and specific failure reasons are needed
  beyond the stage/retry guidance before calling the overview fully accepted.
  Gate future filesystem/download/Python workflows by their own advertised BLE
  support and current agent availability, not just retained USB protocol versions.

Acceptance: refresh never flashes controls or swallows clicks; unsupported or
absent-agent features explain why they are unavailable; identity validation and
flash failure are tested; saved identity survives reboot and USB enumeration.
Verify USB toggles, authentication failure/timeout, physical Y Stop, detach, disconnect,
partial-upload cancellation, near-limit effects and local-preset contention.
Record pending and completed board checks in DEVICE_TESTING.md.

## 2. Stream/filesystem acceptance and throughput

Implementation is complete. Info flag 32 negotiates encrypted Events, independent
stream nonces and four-fragment credits. The relay owns one 2,048-byte page and
at most eight best-effort buttons; native browsing accumulates at most 512 entries.
Filesystem v1 and the sole USB CDC owner are unchanged. Native list/cancel, page
validation, target-platform navigation and consumed-button exclusions are covered
by portable tests. Generic file-transfer sinks remain unavailable until milestone 4.
See [DEVICE_TESTING.md](docs/DEVICE_TESTING.md) for board measurements and commands.

Remaining acceptance/optimization:

- Exercise Home/Root/Up, breadcrumbs, hidden toggling, folder double-click, selection,
  Copy path, Load more and error recovery in the connected native window. The
  headless real-agent check covers multi-page listing, missing paths, cancellation
  and fresh reconnect; it does not establish interactive rendering or navigation.
- Test physical released-button delivery and confirm menus/Payloads/Stop gestures
  stay local, including physical Y Stop during saturated streaming and HID work.
  Routing exclusions and queue saturation have software coverage. Preserve the
  independent local payload job and measure actual Stop/key-release latency.
- Record slow/unresponsive-agent recovery, interrupted ACK/upload, USB loss and
  repeated browsing/connect/release/acquire cycles under load. One early paginated
  hardware run disconnected; repeated runs passed afterwards. Keep soak coverage
  open rather than attributing an unobserved cause to that interruption.
- Force ATT MTU 23 on hardware when a suitable central is available. macOS currently
  negotiates MTU 124, while all values deliberately remain 20 bytes. Codec tests
  cover default-MTU fragmentation. Evaluate larger fragments only with measured
  gains and an explicit packet-pool/characteristic review; the pool remains eight
  128-byte packets. Do not increase every firmware buffer for throughput.
- Set download-specific credits/timeouts from measurements before milestone 4.
  Maximum 2,048-byte probe payloads currently take roughly 5.4–5.7 seconds
  (about 360–380 bytes/s); directory/probe I/O allows 30 seconds. The host sender's
  existing five-second USB chunk ACK deadline cannot be copied unchanged into a
  future end-to-end BLE download path. Keep transfer backpressure fail-closed.

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
TrouBLE 0.6.0 provides the GATT stack. The outgoing path now uses subscriptions and explicit application credits;
throughput evidence belongs in DEVICE_TESTING.md and does not establish other
platforms or larger-fragment performance. See the
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

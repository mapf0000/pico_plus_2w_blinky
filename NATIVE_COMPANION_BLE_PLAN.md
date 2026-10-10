# Native Rust companion and BLE feasibility plan

Status: proposed implementation plan. No companion app or BLE service is implemented by this document.

## Objective and scope

Build an egui desktop companion that discovers the Pimoroni Pico Plus 2 W,
connects over Bluetooth Low Energy, displays device status, and submits bounded
keyboard effects through the existing USB HID execution service. Use it to
measure feasibility and iterate on the connection and UI without replacing the
WLAN frontend.

The existing access point, HTTP assets, Yew frontend, RustPython Worker,
WebSocket protocol, USB host agent, and hardware presets remain supported.
BLE is an additional controller transport, not Bluetooth HID or a network
interface. The control PC and USB-connected target PC are separate roles; the
companion must never execute target-host commands on the control PC.

First hardware acceptance target: the current macOS development environment
and the actual Pico Plus 2 W/RM2 board. Keep common code portable to Windows and
Linux, but report those platforms as unverified until tested.

### First demonstration

1. Launch a native desktop window without Trunk, a browser, or a Pico WLAN
   connection on the control PC.
2. Scan, select the Pico by its service, connect, and display compatible
   capabilities, firmware identity, USB readiness, and host-agent presence.
3. Authorize control using a physically confirmed pairing flow.
4. Select a supported keyboard layout and send a short fixed demonstration
   string to a disposable editor on the USB-connected target.
5. Submit a bounded delayed effect, cancel it, and demonstrate physical Y Stop.
6. Disconnect/reconnect, verify cleanup and absence of replay, then use the
   existing WLAN UI again without rebooting the Pico.

Discovery/status alone is the first milestone. Keyboard control is a later
milestone gated on session routing and authenticated pairing.

### Deferred scope

- Replacing or deleting the WLAN UI, or disabling its asset build pipeline.
- Native Python execution, a full script editor, and interpreter migration.
- BLE file transfer, host filesystem browsing, credentials, shell commands,
  USB identity changes, and remote firmware updates.
- Persistent bonding, automatic controller takeover, multiple BLE controllers,
  background tray operation, mobile support, and a production installer/updater.

Python remains in the browser Worker. The companion initially produces KBD1
directly through `keyboard-core`; it does not embed RustPython. Any later native
interpreter needs a separate supervised process, bounded IPC, hard termination,
and an explicit isolation design rather than a GUI/background thread.

## Architecture

```text
Control PC                               Pico Plus 2 W
egui UI                                  HTTP/WebSocket adapter <-- WLAN UI
   |                                                 |
companion client core                    shared controller/session services
   |                                                 |
btleplug BLE adapter <-- BLE GATT --> firmware BLE adapter
                                                     |
                                         existing HID executor + USB CDC
                                                     |
                                           USB-connected target PC
                                                 + host agent
```

Choose `egui`/`eframe` for the native window, Tokio for asynchronous connection
work, and `btleplug` for host BLE access. Start with one selectively enabled
renderer; assess `glow` against `wgpu` on the target machine instead of assuming
a particular backend is smallest. No local HTTP server, WebView, JavaScript,
or WASM is needed by the companion. Existing firmware builds still require
their current web tooling.

Use bounded messages between the UI and async backend. The UI owns presentation
state, not Bluetooth objects or protocol state. Repaint on state changes and
scheduled UI needs; never perform scans, writes, or waits inside egui callbacks.
Treat binary size, idle RAM, idle CPU, and startup time as measurements, not
framework guarantees.

### Proposed repository layout

| Location | Responsibility |
| --- | --- |
| `apps/companion/` (package `pico-companion`) | egui window, UI models, settings, CLI, native BLE adapter and backend task |
| `crates/companion-core/` | Host client state machine, request correlation, deadlines, bounded diagnostics, transport interface, deterministic mock transport; no egui or OS Bluetooth dependency |
| `crates/ble-protocol/` | `no_std` BLE service UUIDs, version/limits, fragment codec and reassembly state machine; shared by app and firmware |
| `firmware/src/control/` | Transport-neutral command/effect dispatch, session identity, authorization/admission, remote completion routing |
| `firmware/src/ble/` | CYW43/TrouBLE adapter, GATT callbacks, pairing, buffers and Embassy tasks |
| Existing shared crates | Reuse `keyboard-core`, `script-protocol`, `bytecode-constants`, and `firmware-exec` |

These are working names. Keep `companion-core` small: extract reusable behavior,
not an all-purpose networking framework. Introduce shared command DTOs only
where both endpoints use them; do not move Yew/browser dependencies into shared
code. A future native WebSocket adapter can fit the same interface, but is not
required for BLE feasibility.

Add the app and crates as workspace members without changing root
`default-members` or introducing a root default target. Companion builds must
not depend on the firmware package or invoke firmware asset generation. Use
`MIT OR Apache-2.0` for new reusable crates. Resolve dependencies narrowly and
review real lockfile changes.

## Existing coupling and planned refactorings

### Command service, with an unchanged WebSocket wire contract

`firmware/src/http/routes/ws.rs` currently parses `RPC <id> <command>`, handles
commands, validates binary script effects, and formats replies. Extract the
command/effect application logic into `control/`, leaving socket reads/writes,
WebSocket close behavior, legacy text parsing, and wire formatting in the
adapter. Move protocol-neutral status snapshots and JSON utilities out of HTTP
ownership where necessary; do not copy command implementations into BLE.

Use typed internal requests/results and an explicit per-transport capability
allowlist. Initial BLE operations are HELLO, STATUS, acquire/release control,
run effect, and cancel effect. Disabled operations return a defined unsupported
result, even if the WLAN dispatcher supports them. Preserve the existing
WebSocket version and payloads; BLE has its own transport version.

### Session identity and controller ownership

`firmware/src/http/transfer.rs` currently owns browser generations, replacement,
queue draining, and disconnect-triggered HID cancellation. Extract controller
identity/lifetime into a transport-neutral session service. Each remote owner
must identify both transport and connection incarnation. Never compare a BLE
connection ID directly with a browser generation; prevent identity reuse while
old work can remain queued.

Use one active remote control lease for the feasibility phase:

- Read-only BLE discovery/status does not acquire the lease or drain WLAN
  queues. A BLE status connection can coexist with a WLAN owner.
- Opening a WLAN control session while another WLAN session owns control keeps
  the existing newest-WebSocket replacement behavior.
- BLE acquisition while WLAN owns control returns busy. A WLAN connection while
  BLE owns control is refused with the existing frontend-recognized replacement
  close code `4001`, so it pauses reconnect rather than displacing BLE. Record
  the busy reason in device/app diagnostics. HTTP assets remain available.
- The operator releases/disconnects the current controller before switching
  transport. No automatic cross-transport preemption or takeover is added.
- Local hardware presets continue to use the same non-preempting HID service.
  A remote lease does not prevent local preset admission when the executor is
  idle. Competing HID submissions still return busy.

Authentication precedes BLE control acquisition. Disconnect/release invalidates
that session, cancels only its remote work, drops its incomplete messages and
pending replies, and leaves local jobs and another owner's state intact. Old
session guards must not clear the new owner. Reconnection negotiates a new
session; queued keyboard commands are never automatically replayed.

### HID completion routing

`firmware/src/usb/hid.rs` currently imports HTTP transfer types, names its remote
owner `Browser`, and sends completions through
`transfer::send_text_for_generation`. Generalize remote ownership and publish
typed completions through the controller service. Serialize them into the
existing JSON event for WLAN and the corresponding BLE response for BLE.

Preserve one-job admission, strict two-pass validation, reservation through key
release cleanup, one terminal completion, independent local results, scoped
remote cancellation, and physical Y Stop. A stalled BLE notification consumer
must not hold HID admission forever: delivery has a finite deadline, after
which the BLE session is closed, its reply discarded, and the completed
reservation released. USB/HID cleanup must progress independently of BLE I/O.

### Keep the bulk transfer pump owned by WLAN

Do not make the existing 16-event transfer queue a competing BLE consumer.
Keep USB file-transfer ACK/backpressure, secure session negotiation, the PSRAM
batch slot, and the WebSocket output pump on the WLAN data path. BLE receives
only explicit controller replies/status and its own HID results. Do not
broadcast credentials, file contents, or host results to BLE observers.

While BLE holds the lease, unsupported filesystem/secure-transfer requests
remain disabled and the absence of a WLAN transfer owner follows the current
relay rejection/abort behavior. A lease switch must invalidate any old WLAN
transfer session before BLE is admitted; no transfer is silently migrated.

## Pico BLE implementation

### Board integration and feature boundary

Add an opt-in firmware feature named `ble`, requiring `firmware` and activating
the CYW43 Bluetooth feature plus a compatible `trouble-host` dependency. Leave
default WLAN builds intact. The cached `cyw43` 0.7.0 source already exposes
`new_with_bluetooth` behind its Bluetooth feature, uses `bt-hci` 0.8, and the
repository contains `43439A0_btfw.bin`. Choose a TrouBLE release whose HCI traits
match that dependency; duplicate incompatible `bt-hci` versions cannot bridge
the controller traits. Verify this with an embedded compile before adopting
sample code or changing versions.

In the BLE feature branch of `main.rs`, load the aligned Bluetooth blob and
initialize CYW43 once using `new_with_bluetooth`. Keep its returned network
device, the existing CYW43 runner, CLM setup, AP startup, board pins, RM2 SPI
divider, and DMA allocation. Feed the returned Bluetooth driver to the BLE
host. Use statically allocated resources and bounded Embassy tasks; do not
start a second radio driver or add a firmware heap.

The upstream Pico W example is evidence for the integration approach, not proof
of coexistence or security on this RP2350B/RM2 board. Verify AP+BLE behavior on
the actual hardware early. Once radio initialization succeeds, later BLE host
errors should release BLE state and leave WLAN usable; report any initialization
failure that cannot safely recover instead of promising an unverified fallback.

### GATT service and message transport

Use a custom, versioned GATT service with fixed project UUIDs shared through
`ble-protocol`:

| Characteristic | Purpose |
| --- | --- |
| Info (read) | Compact transport version, limits and service identity; exclude credentials and secrets |
| RX (write with response) | Fragments of bounded commands/effects |
| TX (notify) | Fragments of replies and completion events |
| Control (write with response) | Small acquire/release, message receipt, cancel and liveness operations; serviced independently of effect reassembly |

Use writes with response first; defer write-without-response windows and L2CAP
CoC. Subscribe to TX before negotiating the application session. Do not assume
that the OS exposes an ATT MTU query or that a large GATT value fits one packet:
support the 20-byte characteristic payload at default ATT MTU 23, and increase
fragment size only using a tested safe platform limit.

Define a BLE v1 fragment header with version, message kind, connection/session
binding, message ID, total length and offset/sequence. Freeze the exact byte
layout and UUIDs in `docs/PROTOCOL.md` before implementing both endpoints. Keep
the header small enough to carry data at MTU 23. Use checked little-endian
decoding, exact final lengths, and one active reassembly per direction.

Initial logical payloads reuse the existing capability/status JSON and full
`script-protocol` run/cancel envelope as opaque application bytes. Retaining
its leading kind byte avoids changing script protocol v1 just for BLE. BLE
message kinds are a separate namespace, not new USB TLV tags or reinterpreted
WebSocket kinds. Responses retain effect/request IDs and distinguish transport
acceptance, job admission, and terminal execution result.

The logical message ceiling is `script_protocol::MAX_MESSAGE_LEN` (currently
4,125 bytes: 29-byte envelope plus 4,096-byte KBD1), not 4,096 or the USB TLV
2,048-byte limit. Apply smaller per-kind bounds to text replies (currently
768 bytes) and control messages. Test the full KBD1 boundary over small fragments.

Reject unsupported versions, invalid kinds, zero/oversize lengths, gaps,
overlaps, conflicting duplicates, wrong-session messages, and trailing data.
Define identical duplicates explicitly rather than executing a retransmission.
Expire partial messages after a configurable finite deadline (initial target
5 seconds). On timeout, disconnect, release, or protocol failure, reset
reassembly and publish an actionable error.

An ATT write acknowledgement confirms the write, not execution. A successful
notification enqueue also does not prove application receipt. Use one logical
outgoing response at a time with an application receipt through Control and a
finite deadline; terminal HID results are not dropped to make room for status.
Bound retries and receipt deduplication, never retry execution after an
ambiguous timeout. Cancel/lease teardown bypass a partial large message so Stop
cannot be trapped behind reassembly or a saturated data queue.

Keep each Control write within the default 20-byte value limit. Bind it to the
authenticated connection and use a compact session-local message/job token to
resolve the full effect IDs; do not try to fit the 29-byte script cancel
envelope into one Control write. Cancellation of a partial upload drops its
reassembly, while cancellation of an admitted effect uses the shared scoped
HID cancellation service. Include token reuse/stale-token tests.

### Authentication and physical interaction

The read-only radio spike exposes only a synthetic ping and a minimal device
snapshot. Before allowing keyboard actions, verify authenticated encrypted LE
Secure Connections with numeric comparison or passkey entry supported by the
chosen TrouBLE version, CYW43 controller, and macOS Bluetooth stack. Use the
Pico display/buttons to confirm pairing during a short, explicitly entered
pairing window; keep control inaccessible until that succeeds. Display the
connection/control owner and pairing outcome using existing display models.

Just Works encryption alone does not authenticate the controller. Discovery
names, RSSI, UUIDs, and a local confirmation button alone are not peer identity.
If the authenticated flow cannot be demonstrated, stop at read-only feasibility
and document the missing security capability before designing another scheme.
Do not invent an ad-hoc shared-secret authentication protocol to unblock HID.

Start with a bounded volatile bond store and no flash layout/config schema
change. Document explicit re-pairing after Pico reboot and OS stale-bond removal;
test forget/re-pair behavior. OS permission prompts may need a development app
bundle and platform metadata even before installer work. Never log pairing keys,
typed text, bytecode contents, credentials, or transferred data.

### Memory and execution budget

Produce a capacity table before merging the BLE connector: CYW43 Bluetooth
state, TrouBLE packet pool/MTU, connection and bond counts, GATT attributes,
reassembly buffers, replies, channels, task frames/stacks, and code/blob growth.
Start with one BLE connection, one pending request/effect, one RX reassembly,
one bounded outgoing logical response, and a reserved small control path.

Reuse buffers by ownership where practical rather than queueing multiple
4,125-byte messages. Account for the existing 4,096-byte HID program copy and
all enum/task storage; one slot is still a real SRAM allocation. Do not size the
BLE pool using WLAN's 32 KiB WebSocket window. Keep PSRAM allocation policy,
flash config slots, and `memory.x` unchanged unless measured evidence requires
a separately reviewed change. Review the final linked release size and SRAM
headroom with both WLAN and BLE enabled, including key cleanup and watchdog
behavior under saturation.

## Companion behavior

Provide four compact areas: connection/device selection, status/capabilities,
keyboard demonstration controls, and bounded diagnostics. Show adapter missing,
Bluetooth off, permission denied, scan empty, connecting, pairing, incompatible,
busy, ready, reconnecting and disconnected states distinctly. Gate Send on
compatible protocol, authenticated control ownership, a selected supported
layout, and USB readiness. Keep cancellation accessible while a job is active.

The backend owns adapter discovery, service discovery, subscription, session
negotiation, serialization, fragment writes, receipts, request timers and
reconnect. Identify candidates by service UUID, not name or a presumed stable
MAC address; macOS identifiers and privacy addresses need platform-neutral
handling. Stop scanning when finished and clean up subscriptions/tasks on
disconnect and application exit.

Use bounded channels and a finite diagnostic ring; coalesce redundant status
snapshots and show dropped diagnostic counts. Keep terminal replies reliable.
An initial 1 Hz status poll is sufficient; expose measured RTT and failures
without sending high-rate logs. Timeout values belong in one configuration
model and should be adjusted from hardware results, especially at MTU 23.

Reconnect with bounded backoff to an explicitly selected device while reconnect
is enabled. Clear pending effects, request IDs/state and stale callbacks by
connection incarnation. Do not automatically reclaim a released lease, resume
keyboard jobs, or replay text. Store only harmless preferences initially; key
material belongs to platform/firmware security mechanisms.

For the keyboard demo, enable only matching advertised layout features and
lower explicit text/key/delay actions with `keyboard-core`. Preserve its text,
operation and bytecode limits. Send the resulting KBD1 through the same strict
firmware validator/executor as the browser; never encode raw HID reports in the
companion. Provide a fixed harmless demo, a short text field, layout selection,
Send, and Cancel. Python editor parity is a separate project.

## Incremental implementation sequence

Each milestone should be a small reviewable change with its own validation and
recorded result. App shell work and the read-only radio spike do not require a
complete firmware command refactor.

| Milestone | Deliverable | Acceptance / next gate |
| --- | --- | --- |
| 0. Baseline and dependencies | Record current WLAN/HID behavior and linked memory; select matching egui, btleplug, TrouBLE/HCI versions; document macOS permissions/pairing support | Existing targets compile; real board and controller test setup identified |
| 1. Native shell and mock | `pico-companion` window plus `companion-core`; deterministic device/status/effect mock and `--mock` mode | UI remains responsive through errors, timeouts and reconnect; no firmware/Trunk build required |
| 2. Read-only radio spike | Opt-in Pico BLE initialization, service discovery and synthetic ping; real native scan/connect/status | AP/HTTP/WS still work while BLE is connected; repeatable real-board connection without control-PC WLAN |
| 3. Shared firmware service | Extract dispatcher/session ownership and remote HID completion routing; adapt WLAN first | Existing WLAN wire contract, transfer backpressure and local preset behavior preserved; ownership/routing tests pass |
| 4. Bounded BLE protocol and pairing | Freeze BLE v1; shared codec/reassembly; receipts, timeout/cancel control path; physically confirmed authenticated pairing | MTU-23 boundary/malformed tests pass; unauthorized control rejected; measured buffer and linked-memory budget acceptable |
| 5. Keyboard demonstration | BLE control lease, KBD1 submission/completion and cancellation exposed in egui | Real target receives expected text; busy/local-preset/cancel/Y Stop/USB detach/reconnect scenarios behave correctly |
| 6. Feasibility report | Demo instructions, metrics, platform results and defects with follow-up decisions | Explicit go/no-go for further native workflows; no implication of WLAN replacement or transfer parity |

Do not expand the UI or start bulk transfer/interpreter migration before the
radio, ownership, pairing, and HID demonstration gates pass. If board radio
coexistence fails, keep the native mock usable and record the hardware blocker;
do not silently disable WLAN to declare feasibility.

## Validation and demonstration evidence

### Automated checks as implementation lands

Run from the root with package-specific targets. Proposed package commands
become valid only after those packages exist:

```sh
cargo fmt --all -- --check
cargo test -p companion-core -p ble-protocol
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo build -p pico-companion --release
cargo check -p ble-protocol --target thumbv8m.main-none-eabihf --no-default-features
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo test -p firmware-exec -p build-support -p script-protocol
cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
cargo test -p frontend --target wasm32-unknown-unknown --no-run
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf --features ble
```

Make the pure controller/session state machine host-testable through the
firmware library, without RP globals, BLE hardware or generated assets. Test
adapter dispatch separately at its narrowest seam. Add framing tests for
MTU 23 and larger safe values, exact maximum messages, every truncation,
duplicates, gaps, timeouts, conflicting IDs, invalid versions and reset.
Exercise lost receipts, stalled notification consumers, bounded queues,
cross-transport busy/replacement, stale completion/cancel, and local jobs during
remote disconnect. The companion mock should use the shared codecs so it cannot
hide incompatible encodings; it must not be presented as a hardware emulator.

Use `tools/mock-pico` where useful to confirm unchanged WLAN behavior without
inventing a second WebSocket mock. Run connected-browser regression tests for
affected session/HID behavior using the available WebDriver setup. If the
refactor touches USB relay/secure-transfer code, add the host-agent and
`transfer-crypto`/`transfer-protocol` suites from `AGENTS.md`; run Trunk and Worker
checks only when their affected sources/build inputs require them.

### Explicit device test session

Hardware flashing belongs to an explicitly scheduled device-test step when the
board and target are available, not routine validation of this plan. During it:

- Test discovery, permission denial, pairing reject/accept, unauthorized writes,
  Pico reboot, Bluetooth off/on, forgotten bonds, app kill and clean exit.
- Run 20 connect/disconnect cycles and an initial 30-minute AP+BLE soak; record
  errors, reconnect time, watchdog resets and WLAN responsiveness.
- Measure at least 100 synthetic ping round trips; report p50/p95/max RTT,
  packet size, OS/hardware and radio conditions. Use p95 below 250 ms as an
  initial interactive-control target, not an assumed BLE guarantee.
- Test a valid near-4 KiB effect at default/small fragments, partial upload
  cancellation, device busy, local preset contention, physical Y Stop, USB
  detach mid-job and notification stalls. Verify keys release and subsequent
  jobs become admissible; successful transport upload alone is not success.
- While WLAN owns control, BLE status remains available and lease acquisition
  is busy. While BLE owns control, a WLAN tab cannot steal it or receive BLE
  results. On release, WLAN commands, Python effects and a secure file transfer
  work again, with no stale output crossing sessions.
- Record companion stripped release size, startup time, idle/connected RAM and
  idle CPU. Record baseline versus BLE-enabled linked firmware flash/static
  SRAM, task/pool capacities and remaining headroom. Explain renderer/build
  settings; measure before choosing an installer/runtime footprint target.
- List Windows/Linux compilation and physical BLE results separately from
  macOS results. Identify untested browser/hardware/platform cases explicitly.

## Documentation and final feasibility decision

Update `README.md` with actual companion run/demo commands when implemented,
`docs/ARCHITECTURE.md` with service/transport ownership, `docs/PROTOCOL.md` with
BLE v1 and compatibility, `docs/HARDWARE.md` with measured resources/radio setup,
`docs/THREAT_MODEL.md` with the trusted control PC and BLE authentication boundary,
and `docs/DEVICE_TESTING.md` with the acceptance checklist. Update `AGENTS.md`
for new package locations and validated checks. Keep `docs/SCRIPTING.md` clear
that Python still runs in the browser until a separately implemented change.

Feasibility is established when the real native app controls the actual Pico
through an authenticated, bounded BLE path without control-PC WLAN, survives
the defined failure cases, and the existing WLAN UI remains operational under
the documented single-controller policy. A mock-only demo or unauthenticated
read-only connection establishes a narrower milestone, not the full result.

Decide next work from measured evidence: improve BLE reliability/packaging,
add a native WLAN fallback, or extend specific native workflows. Treat BLE file
transfer and native scripting as separate designs. Retiring WLAN assets or
frontend code requires an explicit later decision and feature parity review.

## Reference material

- Repository ownership and constraints: [AGENTS.md](AGENTS.md),
  [architecture](docs/ARCHITECTURE.md), [protocol](docs/PROTOCOL.md),
  [hardware](docs/HARDWARE.md), [scripting](docs/SCRIPTING.md), and
  [threat model](docs/THREAT_MODEL.md).
- [eframe documentation](https://github.com/emilk/egui/blob/main/crates/eframe/README.md):
  native window/rendering integration and renderer choices.
- [btleplug project](https://github.com/deviceplug/btleplug): native async BLE
  central implementation and platform setup.
- [TrouBLE documentation](https://embassy.dev/trouble/): CYW43 host integration,
  static resource configuration, GATT, and authenticated pairing. Confirm APIs
  against the selected dependency version during implementation.

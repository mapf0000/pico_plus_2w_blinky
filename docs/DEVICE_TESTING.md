# Device validation

`scripts/device-test` runs hardware USB checks that do not send keyboard effects. The suite is opt-in and is not part of normal `cargo test` because it requires exclusive access to one physical board.

Measured throughput results, conclusions, and the remaining experiment backlog are recorded in [`THROUGHPUT_EXPERIMENTS.md`](THROUGHPUT_EXPERIMENTS.md).

## Safety boundary

The default command does not flash or reset the board. Neither the default command nor `--flash`:

- sends USB HID reports;
- writes to the mass-storage volume;
- opens, reads, names, compresses, or transfers a host file;
- invokes shell, filesystem, or credential commands;
- starts a Wi-Fi connection or WebSocket client;
- prints USB descriptor strings/serial numbers, ciphertext bytes, file contents, credentials, or keys.

The CDC security checks send fixed synthetic protocol bytes only. The valid `FILE_OPEN` envelope contains a dummy ciphertext-shaped value, not host data. It is expected to receive `FILE_ABORT` reason 5 because no browser WebSocket relay exists. An unexpected ACK is treated as evidence of an active browser and fails the test; the harness sends a bounded cleanup abort.

The optional USB throughput benchmarks also use deterministic synthetic bytes only. The framed benchmark counts frames and bytes, maintains a wrapping checksum, and returns device-side elapsed time. The raw benchmark counts bytes and USB packets without retaining, checksumming, TLV-decoding, or forwarding the stream. Each variant is bounded by `--benchmark-mib` (1–64 MiB), and raw mode exits with an error after five seconds without an OUT packet.

After raw fault injection releases the control port, the real host-agent binary runs with `--device-self-test`. This mode uses production port selection, asynchronous serial I/O, and TLV framing, but it never enters the general dispatcher or reconnect daemon. It has no handlers for shell execution, credentials, filesystem browsing, secure-session negotiation, or file transfer. It sends a fixed `device-self-test` status identity instead of reading or transmitting the machine hostname, and it bypasses the persistent port cache.

Serial ports are opened exclusively. Stop the host agent before running the suite. The runner never kills another process to obtain a port.

## Commands

Run against already installed firmware:

```sh
scripts/device-test
```

Build, verify-flash, execute, wait for enumeration, and test:

```sh
scripts/device-test --flash
```

The flash step uses `picotool load -u -v -x -t elf`, the same verified update load as `scripts/pico-run`. The ELF contains no sections in the final two 4 KiB persistent configuration slots, so this does not perform a full-chip erase or intentionally overwrite those slots. Put the board in BOOTSEL mode before this command when the running application cannot be reset by `picotool`.

Useful focused invocations:

```sh
scripts/device-test --list
scripts/device-test --port /dev/cu.usbmodem12302
scripts/device-test --skip-host-agent
scripts/device-test --usb-throughput-benchmark --benchmark-mib 4 --skip-host-agent
scripts/device-test --usb-raw-throughput-benchmark --benchmark-mib 4 --skip-host-agent
scripts/device-test --flash --elf target/thumbv8m.main-none-eabihf/release/pico_rust
```

Run the hardware-independent harness tests directly:

```sh
cargo test -p device-test
cargo clippy -p device-test --all-targets -- -D warnings
cargo test -p host-agent
cargo clippy -p host-agent --all-targets -- -D warnings
```

Use `--vid` or `--pid` only when the corresponding firmware descriptor/build setting was deliberately changed. `--port` is validated against the selected VID/PID by the raw phase before it is opened by the host agent. The host-agent phase performs two keepalive round-trips at the production 10-second cadence by default; `--host-keepalives`, `--host-interval-ms`, and `--host-timeout-ms` provide bounded diagnostic overrides.

## Automated checks

The raw Rust runner performs these checks first:

1. At least two matching CDC ports enumerate for the composite USB device.
2. Exactly one candidate responds to an empty tag-7 probe with tag 2 `probe-ok`; port numbers are never assumed.
3. The control CDC opens exclusively with 115200/8-N-1 settings.
4. The tag-7 `handshake` response is tag 2 `handshake-ok`.
5. A TLV frame split into individual writes is decoded correctly.
6. Two TLV frames in one write are decoded in order.
7. An unknown tag is isolated and a subsequent health probe succeeds.
8. A truncated version-2 `FILE_OPEN` is silently rejected and the control task remains healthy.
9. A full-length `FILE_OPEN` carrying legacy protocol version 1 is also rejected.
10. A structurally valid encrypted chunk for an unknown transfer receives abort reason 4.
11. A structurally valid encrypted open with no browser receives abort reason 5 and the safe `secure browser relay unavailable` detail.
12. The firmware decoder recovers from an over-limit TLV header and a bounded false frame without a reset.
13. A final probe confirms that all negative cases left the control path responsive.

With `--usb-throughput-benchmark`, the raw runner additionally compares the production TLV/CDC ingress path at ACK cadences 1, 4, 8, and 16 using windows 8, 16, 16, and 32. Every variant validates firmware frame/byte counters and checksum before reporting host- and device-timed KiB/s. A post-benchmark probe confirms continued control-path responsiveness.

With `--usb-raw-throughput-benchmark`, the runner compares raw CDC OUT ingestion using host write sizes of 64, 512, 2,048, and 16,384 bytes. The firmware bypasses TLV decoding and checksum work, reports full/short USB packet counts, and automatically returns to normal TLV mode after the declared byte count. A post-benchmark probe confirms recovery of the normal control path.

The wrapper then starts the real host-agent executable in its restricted one-shot mode and verifies:

1. Production port enumeration/probing selects the control CDC without reading or writing the cached-port file.
2. Production asynchronous serial tasks and TLV framing complete the handshake.
3. A valid agent-status payload with the fixed test identity is sent without exposing the hostname; tag 8 has no explicit firmware ACK.
4. Two subsequent empty tag-7 keepalives receive `probe-ok` at the normal 10-second interval, confirming continued control-path liveness.
5. The process exits successfully instead of entering the reconnect daemon.

Unit tests additionally inject a device-side shell-command tag and verify that restricted mode rejects it without invoking the general dispatcher.

USB mass storage is no longer exposed. The wrapper has no mounted-volume phase
and no `--msc-label` or `--skip-msc` options. Verify the absence of a mass-storage
interface during the board smoke test and use the CDC installation workflow to
check artifact delivery.

Every assertion prints `[PASS]`, `[FAIL]`, or `[SKIP]`, and any failure produces a nonzero process exit status.

## Connected-browser and security validation

The suite intentionally cannot prove the successful browser-to-host security path. The following require a Wi-Fi client, the Web UI, and its WebSocket:

- unattended session negotiation and the Noise handshake;
- browser-generated in-memory session master delivery;
- successful AEAD manifest/chunk/close decryption;
- IndexedDB staging, final SHA-256 verification, and the authenticated browser receipt;
- real USB-to-WebSocket backpressure and browser-disconnect cleanup;
- a successful end-to-end encrypted file transfer.

These require a connected-browser suite or an explicitly reported manual test. They should not be simulated here in a way that weakens the production encryption boundary. The local Firefox asset/Worker smoke test recorded in [HARDWARE.md](HARDWARE.md#16-mib-xip-flash) exercises the release bundle, but does not establish end-to-end Pico Wi-Fi transfer coverage.

### Scripting coverage

The dependency upgrade on 2026-10-08 recorded all 11 native RustPython VM tests passing and the same suite passing on the release wasm target in headless Firefox. Those tests cover generator state across 100 steps, send/throw, all injected exception classes, invalid yields/missing layout, source bounds, UTF-8 diagnostic truncation, syntax diagnostics, codec initialization, and denied imports. The current tests are in `apps/python-worker/src/lib.rs`; browser execution uses `run_in_browser` with an installed WebDriver/browser. Temporary machine-specific driver paths from that run are not setup requirements.

That VM suite does not exercise the Yew supervisor or physical HID. Separately verify lazy asset loading, hard timeout and recovery from a non-yielding loop, single-process/one-effect ownership, stale process/effect results, matching event waits, disconnect/reconnect continuity, no effect retry, and Stop teardown. Hardware HID checks must use a safe capture target and cover cancellation during key/modifier-down, delay, USB writes, reconnect key release, local/browser job admission, and generation-owned completion. These checks are not part of the default USB suite; physical cancellation latency and peak stack use remain unmeasured.

### Transfer/security coverage

Current unit tests in `crates/transfer-crypto`, `crates/transfer-protocol`, and the host/frontend transfer modules cover Noise transport, record type/index authentication, manifest/close codecs, malformed bootstrap secrets, exact chunk/TLV bounds, batch bounds/trailing data, and streaming ciphertext with a browser receipt. Host PTY tests exercise general daemon/serial behavior; they do not prove a complete physical encrypted file transfer.

The former implementation plan required the following acceptance coverage. Preserve these as checks to confirm or extend, rather than assuming that implementation or compilation proves them all:

- Tampered ciphertext/authenticated metadata, wrong keys, record-domain separation, malformed/oversized/trailing envelopes, and stale session/transfer identities.
- Empty files, exact maximum chunks, source length changes, out-of-order/duplicate chunks, ACK regression/future offsets, retry ciphertext identity, cancellation, and mismatched/missing browser receipts.
- Disconnect/reconnect under queue pressure, real USB-to-WebSocket backpressure, browser decryption/hash/persistence failures, and bounded receiver resource use against a hostile source PC.

Use [AGENTS.md](../AGENTS.md#validation-matrix) for reproducible build/test commands and [THREAT_MODEL.md](THREAT_MODEL.md#receiver-defenses-and-review-priorities) for the security review scope. Frontend browser integration tests and Linux/Windows builds were not run in the recorded dependency upgrade; that gap must not be inferred closed from macOS or Worker-only tests. Independent cryptographic review, fuzzing, and sustained connected-browser/hardware tests remain follow-up work.

## CDC installation checks

Run the host-side installer integration suite on native Apple Silicon macOS:

```sh
scripts/test-cdc-installer
```

This suite uses temporary PTYs and a compiled descriptor-audit helper as its
downloadable executable; it never touches physical USB, sends HID reports, or
starts the real agent. It covers fragmented installer and binary delivery,
fullblock/byte readers, size/digest verification, detached descriptor closure,
corruption, malformed metadata, symlink rejection, early EOF, watchdog expiry,
interrupt cleanup, staging cleanup, and child-process cleanup. The serial glob
is substituted with a temporary test path; watchdog expiry uses a shorter test
interval. These substitutions do not change the receiver protocol.

The existing `scripts/device-test` safety boundary remains unchanged. CDC
installation hardware checks are separate and explicitly opt-in: they flash
firmware, receive an executable, modify an installation directory, and launch
an agent. Stop an existing agent first. Provision a unique `PICO_USB_SERIAL`
and build/package from source, then select the manual-arm action on the board
and enter the receiver command from README. Verify exact artifact equality and
a real TLV handshake, then test repeat installation, Y Stop, unplug during
transfer, and return to normal control service. Check physical Terminal/HID
behavior separately on US and DE input layouts. See the
[operator workflow](../README.md#hardware-keyboard-payloads),
[identity requirements](HARDWARE.md#cdc-installer-identity-and-flash-use), and
[stream contract](PROTOCOL.md#explicitly-armed-cdc-bootstrap-v1).

For installer, preset, or bootstrap changes, run `scripts/test-cdc-installer`,
`cargo test -p build-support -p firmware-exec`, the firmware library tests, and
an embedded release build with the provisioned `PICO_USB_SERIAL`. Use the
[validation matrix](../AGENTS.md#validation-matrix) for exact target checks;
include the layout/Worker suites when changing a layout, and browser tests when
changing frontend status. Verify the linked image boundary and single-copy
artifact embedding. All generated files remain under Cargo `OUT_DIR`.

### Recorded CDC acceptance (2026-10-10)

The tested host was Apple Silicon macOS 26.6.2 with zsh 5.9 and `/bin/sh` Bash
3.2.57. These are single-host results, not a supported-platform guarantee.

| Check | Observed result |
| --- | --- |
| Maintained PTY installer suite | All eight scenarios passed, including both exact readers, descriptor audit, corruption, manifest/symlink rejection, EOF, and watchdog/interrupt cleanup |
| Native bootstrap/display, generator/executor, and layout tests | Passed, including malformed/fragmented requests, extent reconstruction, admission, cached-presence retry guidance, and German redirection mapping |
| Embedded build/Clippy and frontend checks | Passed; frontend wasm tests compiled, Trunk release embedding succeeded, and the additive bootstrap-status test ran in headless Firefox |
| Manual-arm production installation | All 1,487,440 bytes matched the packaged artifact's size/SHA-256, and the real agent handshook; 2.574 seconds using a programmatically launched receiver, without HID |
| Physical German CDC install | Terminal received the exact 96-character command; artifact size/SHA-256 and real agent handshake matched; 9.407 seconds from arm to handshake, including launch/typing |
| Same-port physical reconnect | Provisioned logger/control serial names survived reconnect |
| CDC-only composite after MSC removal | IORegistry showed CDC control/data interfaces 0/1 and 2/3 plus HID interface 4; no class-8 interface or `PICO_AGENT` mount; serial names unchanged |
| `scripts/device-test --port /dev/cu.usbmodemP12345673` after removal | All 11 raw protocol checks, restricted production handshake, and two 10-second keepalive round-trips passed after the operator stopped the agent |

The operator also reported successful repeat installation, cancellation, and
operation after the wait/retry footer and MSC-removal updates. Cancellation
method/latency and exact footer timing were not recorded. A fresh installation
was not separately instrumented after MSC removal. Physical process-group SIGINT
recovery passed on earlier isolated diagnostic firmware before installer
delivery and during a stalled block; it does not establish production Terminal
Control-C or Y Stop behavior. Earlier diagnostic fallback-reader downloads also
matched the real artifact, but did not install or execute it.

Remaining physical acceptance checks:

- Automatic US input-layout delivery and German delivery when macOS classifies
  the Pico keyboard as ISO; the passing German result used ANSI classification.
- Production interruption recovery before complete installer delivery and
  during a binary block, measured Y Stop/Terminal Control-C recovery, and unplug,
  reconnect, and retry while preserving the existing installed executable.
- Multiple attached Picos, unrelated serial devices, alternate USB ports/hubs,
  and other intended macOS versions. Zero/multiple-match refusal has local
  selector coverage; physical multi-device coverage remains outstanding.
- Native Intel macOS delivery requires a matching artifact and implementation;
  the current installer intentionally rejects unsupported architectures.

The first stage has no host watchdog until the complete installer arrives.
Device timeouts and USB ZLPs do not guarantee serial EOF, so a waiting shell may
need Terminal Control-C. Keep this limitation visible when evaluating recovery.


## Historical read-only BLE spike

2026-10-10, Apple Silicon macOS and Pimoroni Pico Plus 2 W/RP2350B:

- User explicitly authorized flashing/testing after `picotool info` recognized
  the RP2350 ROM device. Built `ble usb_autostart`; `picotool load -u -v -x -t elf
  /tmp/pico-ble-smoke.elf` completed verification and reboot. Persistent flash
  configuration slots were outside all load segments.
- USB CDC logger enumerated and reported BLE advertising and `PicoEndpoint` AP
  startup. The native window was launched; the user confirmed connection.
- Initial headless discovery timed out before the GUI/OS permission path was
  exercised. Subsequent headless discovery worked. Initial reconnect exposed
  CoreBluetooth invalidating old peripheral handles; connection now rediscovers
  on every attempt instead of reusing a disconnected handle.
- `cargo run -p pico-companion -- --ble --self-test` passed against the board:
  discovery, validated information, three periodic status reads with advancing
  uptime, clean disconnect and reconnect. Read status showed USB enabled/ready
  and host agent absent. Latest application-level request RTT was 96 ms.
- `target/release/pico-companion --ble --self-test` passed again with 97 ms latest
  status-read RTT. These include the backend's 50 ms polling and are individual
  readings, not a percentile/performance benchmark or actual radio-only latency.
- `target/release/pico-companion --mock --self-test` passed. The native BLE window
  was reopened for continued iteration. All BLE application traffic was reads;
  no keyboard/host command was sent.
- A direct HTTP probe from the Mac timed out because it was not connected to the
  Pico AP. User deferred second-device HTTP/WebSocket coexistence checks and
  stated these should not gate further BLE work; WLAN was enabled at that historical milestone and has since been removed.
  AP startup alone does not establish HTTP/WebSocket or transfer performance.

Automated/build validation for this milestone:

```sh
cargo fmt --all -- --check
cargo test -p ble-protocol -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf --features ble
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf --features 'ble usb_autostart'
```

All passed: 20 native/protocol tests and 30 firmware host tests. The normal
stripped companion release was 7,825,264 bytes (7.46 MiB). Firmware links kept
flash/SRAM within their existing regions. The existing Python Worker compressed
size warning remains; TrouBLE's proc-macro dependency also reports a Rust future
compatibility warning. No existing locked dependency version was removed or
upgraded; 18 new lock entries include the local codec and BLE platform graph.

Still unverified: Windows/Linux, forced MTU-23 hardware negotiation, Bluetooth
off/on and denied-permission recovery, long reconnect/soak and RTT percentile
measurements. Authenticated pairing, control ownership, real HID, abort/upload
and Y Stop scenarios belong to subsequent command/pairing milestones. Read-only
BLE success does not validate those security or execution paths.


## Bluetooth-only control milestone — 2026-10-10

User scope changed to remove WLAN and implement Bluetooth. Removed AP/network,
HTTP/WebSocket, Yew assets, RustPython Worker, Trunk build pipeline and obsolete
WebSocket test tooling. USB wire tags and flash reservations are unchanged.
Bluetooth v2 is a deliberate break from the read-only v1 service.

Built default firmware and verify-flashed with:

```sh
picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/pico_rust
```

Verification and reboot passed. ELF load segments exclude both persistent slots.
The four-MiB image remains at `0x10BFE000`; application load ends at `0x100A1D78`.
Linked allocated flash sections total 4,857,200 bytes (including internal image).
Static striped SRAM ends at 92,508 bytes, leaving 431,780 before runtime stacks.
The preceding read-only WLAN+BLE build used 9,862,860 flash section bytes and
263,552 static SRAM: reductions of 5,005,660 and 171,044 bytes respectively.
The stripped native release is 7,858,720 bytes (7.49 MiB); baseline was 7,825,264.
These are linked sizes, not peak runtime memory or measured idle CPU/RAM.

Real macOS/Pico BLE self-test passed: discovery, protocol-v2 info, three advancing
status reads, disconnect and reconnect. Latest status read was 104 ms, with USB
enabled/ready. This test sends no control writes or HID effects.

User-confirmed hardware acceptance: pairing/control acquisition, actual demo
text output into the USB target editor, and cancellation during the initial
delay. The updated native window was launched. Pair rejection/timeout, USB
controls, upload cancellation, physical Y Stop, local contention and detach/
re-pair edge cases remain to be exercised. No persistent bonding; each
connection requires pairing. Trouble host logging is disabled because upstream
security diagnostics include pairing codes/bond material.

Automated checks passed: 44 native/protocol/executor/build-support/script tests,
31 portable firmware tests, 30 host-agent tests (including macOS PTY), nine
transfer tests, four layout tests and five device-tool tests. Targeted Clippy,
formatting, mock self-test and both release builds passed. Commands:

```sh
cargo fmt --all -- --check
cargo test -p companion-core -p pico-companion -p ble-protocol -p firmware-exec -p build-support -p script-protocol
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
cargo test -p host-agent -p transfer-crypto -p transfer-protocol
cargo test -p device-test
cargo clippy -p device-test --all-targets -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Coverage includes maximum 4,125-byte reassembly at default ATT MTU, malformed
headers, gaps/replays, partial-upload replacement, priority cancellation during
stalled I/O, old connection reset, USB gating, full keyboard lowering, strict
executor lifecycle and pairing-screen text. Hardware confirmation is not replaced
by these tests. Linux/Windows Bluetooth, forgotten bonds, long soak, physical
key-release/USB detach and near-limit radio uploads remain unverified.

Acceptance sequence: connect, acquire control, compare codes, confirm Pico X,
type demo text into a disposable editor after initial delay, cancel a delayed
job, exercise Pico Y Stop, release/reconnect with no replay, and reject pairing.
Then test USB on/off, partial upload cancellation, local-preset contention and
USB detach during execution. Bulk transfer/native Python remain deferred.

`scripts/device-test --host-interval-ms 1000` passed on the installed
Bluetooth-only firmware: 11/11 raw CDC cases, host-agent handshake and two
keepalives. Encrypted open without a receiver aborted correctly; no HID or host
file access occurred.

Final review added a regression test for Cancel arriving before a queued Run
starts. The actor discards that Run; it cannot execute after the UI reports
cancellation. Native/core suites and targeted Clippy passed again after the fix.


## Unattended BLE v3 authentication prototype — 2026-10-10

Implemented per-device Noise PSK authentication, with no physical pairing gate,
SMP/bond requirement or display overlay. Provisioning lives in a separate two-slot
8-KiB region at `0x10BFC000..0x10BFE000`. The agent image and USB identity addresses
are unchanged. Control requires encrypted Acquire; all operations/results/polled
status stay in directional authenticated records. No early handshake effects.

A private random profile/image was generated locally (ignored `target/provisioning/`),
and a mode-0600 JSON backup was saved in the control user's Pico Companion
application-support directory outside build output. No key appears in Git or logs.
Using the user's earlier flash-and-test authorization, both key-region and firmware
verification passed, then the device rebooted to the application:

```sh
picotool load -v target/provisioning/pico.flash.bin -t bin -o 0x10BFC000
picotool load -v target/thumbv8m.main-none-eabihf/release/pico_rust -t elf
picotool reboot
target/release/pico-companion --profile target/provisioning/pico.json --self-test
cargo run -p pico-companion --example auth_boundary
```

Real Apple Silicon macOS/Pico acceptance passed:

- Fresh Noise authentication, Acquire/Release, encrypted advancing status,
  disconnect and reauthentication/reacquisition, with no Pico button interaction,
  OS pairing prompt or HID effects. Latest encrypted status request was 397 ms.
- A different key with the same device ID failed authentication and disconnected;
  the companion reports “Device authentication failed; check the provisioning
  profile.” It never reached acquired control.
- A plaintext Acquire written to Command before authentication closed the link.
- An unauthenticated client expired despite 493 continuous public status reads:
  29.68 seconds after discovery/read setup, consistent with the 30-second deadline
  from firmware accept. Reassembly/read traffic cannot renew that deadline.
- The newly provisioned key survived the ordinary ELF deployment and was loaded
  on boot. Firmware CDC logs showed advertising/connection recovery and no
  watchdog recovery; the display remained operational.

Initial immediate post-reboot scans reported no device; later rescans succeeded
after startup. The full application-only native window was
launched with a profile; an eframe screenshot was inspected (no desktop capture).
The longer authentication button renders correctly. The previously reported
once-per-second busy-state flicker remains milestone 1 work.

Automated validation passed: 36 native/protocol/session tests, 31 portable firmware
model/display tests and 20 executor/script/transfer tests. Session tests cover Snow
key agreement in both roles, wrong PSK/context, replayed initial handshake followed
by stale Acquire, tampering/reflection/replay, skipped sequence numbers, maximum
records and MTU-23 reassembly/cancel/readback. Profile tests check CRC/header
corruption, private permissions, strict import bounds and refusal to overwrite.
Commands (test counts exclude doc tests):

```sh
cargo test -p ble-session -p ble-protocol -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p ble-session --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo test -p transfer-crypto -p transfer-protocol -p firmware-exec -p script-protocol
cargo check -p ble-session --target thumbv8m.main-none-eabihf
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
```

Linked allocated flash sections are 4,805,132 bytes plus 3,544 initialized-data
load bytes, including the fixed 4,194,304-byte image. Application load ends at
`0x10095FE8`. No ELF load segment overlaps the key reservation or USB identity.
Allocated static striped SRAM is 95,028 bytes; its highest address including
alignment is 95,036 bytes from RAM origin, leaving 429,252 before runtime stacks.
Removing SMP offset the new crypto's code cost: flash section/load total decreased
48,524 bytes from v2; the static RAM end increased 2,528 bytes. The native release is
7,982,592 bytes. These are linked sizes, not peak RAM/stack or standalone crypto
execution-time measurements. Existing upstream `proc-macro-error2` future-incompatibility
notice remains; targeted Clippy has no warnings.

Unverified: independent power cycling, forced MTU-23 hardware negotiation, v3 HID
output/cancel/physical Y Stop, maximum radio upload/interrupted response, local
contention, USB toggles/detach, long soak and Linux/Windows. Earlier user-confirmed
HID/cancellation acceptance was v2 and is not relabelled as v3. The private backup was also used for a second successful authenticated/reconnect
smoke after the rejection tests; latest encrypted status request was 342 ms. The
release companion was opened using that backup. The wire writes
always fit MTU 23; host integration tests prove that bound without establishing
radio throughput. OS credential-store import/profile selection, remote atomic
rotation and native Python/filesystem/file receipt remain unfinished.

## Stable controls and protected overview slice — 2026-10-11

Status/overview reads now use background state rather than the user-operation
busy flag. One bounded user intent waits behind a read, is revalidated after it,
and is discarded on disconnect, failure or timeout (including a late read reply).
Queued text can be cancelled before submission. Diagnostics remain capped at 128
static messages and now carry timestamps, session/operation/error context, with
filtering, errors-only, clear and optional autoscroll. Text, bytecode and keys are
never diagnostic data. The stale BLE v2 label is corrected.

Overview v1 adds a negotiated Info flag and acquired, encrypted ReadOverview.
It reuses host heartbeat/metadata and CDC bootstrap state and reports full build,
execution/USB limits and retained filesystem/transfer versions. The finite response
bound grows from 48 to 304 bytes; ordinary snapshots stay 48. No USB TLV change,
streaming relay, new connection or increased packet/queue pool is introduced.
The companion targets one-second status and ten-second overview polling while
idle. Heartbeat age is sampled at overview time. Older v3 companions reject the
new flag; update both ends. New companions can omit overview on earlier v3 firmware.

The user's existing flash/test authorization was used on the supplied BOOTSEL
device. Flash verification completed before reboot; the key reservation was not
rewritten. Commands:

```sh
picotool load -u -v target/thumbv8m.main-none-eabihf/release/pico_rust -t elf
picotool reboot
```

The updated native self-test was run with the private provisioning backup in the
control user's application-support directory, outside `target/`. First discovery
immediately after reboot found no advertiser; a later scan passed. Apple Silicon
macOS/Pico results:

- Fresh authentication, acquired encrypted overview readback and advancing live
  status passed. Readback reported the full `0.1.0-release` build, 4,096-byte KBD1
  bound, idle CDC stage and absent agent. Latest status read was 395 ms.
- Release sent during an in-flight background read completed successfully;
  the read did not mark UI controls busy or reject the command as queue-full.
- Disconnect and fresh reauthentication/reacquisition/release passed without
  Pico interaction or HID effects; the provisioned key survived this ELF deployment.

Automated checks passed (47 native/protocol/session tests and 31 portable firmware
tests; doc tests excluded):

```sh
cargo test -p ble-protocol -p ble-session -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p ble-session --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

Regression cases cover stable availability over repeated reads, queued single
submission, queue saturation, cancel before upload, fresh USB-state revalidation,
timeout/disconnect/stale results, late completion past deadline, clear during I/O,
bounded diagnostics without typed text, acquired-only/throttled overview and
incompatible metadata discarding queued work. Codec tests cover old/new v3 flags,
absent/stale/maximal metadata, truncation, malformed flags/versions/strings,
trailing data and maximum response read framing at 20-byte ATT values.

Linked flash including initialized-data load is 4,810,592 bytes, an increase of
1,916. Static SRAM is 95,280 bytes, an increase of 252; its aligned end is
`0x2001743C`, leaving 428,996 bytes in striped SRAM before runtime stacks.
Application flash load ends at `0x10096764`; no load segment overlaps control keys
or USB identity. Native release size is 7,999,424 bytes. Increased response/snapshot
scratch is fixed-capacity; linked size does not establish runtime stack headroom.
The existing upstream `proc-macro-error2` future-incompatibility notice remains.

The actual native window was launched and an application-only screenshot inspected
for the diagnostic controls and basic layout. The release window was opened with
the private profile for connected visual acceptance. The user subsequently confirmed
“Buttons stay steady” after connecting and acquiring control across several refreshes.
Connected button flicker acceptance passed. Interactive diagnostic filtering/scrolling,
populated or stale real agent metadata and CDC installation transitions remain
unconfirmed. No v3 HID/physical
Stop, maximum radio upload, USB detach/toggle, independent power-cycle, soak or
Linux/Windows acceptance is claimed here. USB identity editing/persistence remains
the next implementation slice in the updated plan.


## USB identity over encrypted BLE — 2026-10-11

Implemented native manufacturer/product editing and explicit reload, acquired
identity-v1 requests, and verified persistence through the existing version-1
slots. Info advertises capability bit 16 (current flags 28); older v3 firmware can
still be used without the editor. Older companions that reject the new flags need
updating. No UUID, USB TLV, KBD1, key slot, memory map or response capacity changed.
Manufacturer/product bounds are printable ASCII 32/48 bytes, including empty.
Background reads preserve drafts and button availability; failed saves retain the
draft, and flash failures require a fresh read before retry. Saving requires USB
fully stopped and no reserved HID/installer work. USB lifecycle serialization,
stale stop-signal reset, and accurate shutdown/start failure responses prevent an
old acknowledgement from admitting a write during shutdown.

The user explicitly made BOOTSEL available. This release was loaded and verified
with `picotool load -u -v target/thumbv8m.main-none-eabihf/release/pico_rust -t elf`,
then rebooted only after verification completed. Factory key provisioning was
preserved; no full-chip erase or key write was performed. The private profile
backup outside build output was used without printing its contents.

Apple Silicon macOS / Pico hardware checks passed:

- `target/release/pico-companion --profile <private-profile> --identity-self-test`
  acquired control, disabled USB, saved temporary identity, read it from flash,
  enabled USB, repeated stop/start twice, disconnected and freshly authenticated,
  read the same identity, then restored the original names and original USB state.
- During its ten-second hold, IORegistry reported the same USB device with
  manufacturer `Pico BLE test` and product `Identity test 20261011`. After
  restoration, the original `Logger + Keyboard` product enumerated again.
- `target/release/pico-companion --profile <private-profile> --self-test` passed
  encrypted status/overview/identity readback, release during refresh and fresh
  reconnect. Uptime advanced to 105 seconds with no restart; latest status read
  was 403 ms. No HID effect was submitted.

Automated checks passed: 10 protocol, 29 companion-core, 8 native companion and
36 portable firmware tests (83 total, excluding doc tests). Exact commands:

```sh
cargo test -p ble-protocol -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

Fault tests include every erase byte (0–4,096) and every staged/commit programming
cut (0–256), preserving the latest valid slot or a complete new record. They
cover legacy records, slot alternation, CRC, malformed records, empty/maximal
strings, sequence wrap, unchanged-save wear, and failures before and after commit
verification. Client tests cover acquired/USB-off/capability gates, fresh status
revalidation of queued saves, explicit reload after flash failure, stale results
and no replay after timeout. The native decoder checks authenticated correlation.

Flash safety was reviewed against Embassy RP 0.10.0: ROM/RAM helpers, SRAM write
buffers, interrupt/core-1 exclusion and XiP DMA draining. Linked `write_flash_inner`
is at `0x20000DC0`. One 4-KiB erase and two 256-byte programs ran successfully with
the BLE stack active and USB stopped. Linked flash including initialized data is
4,823,380 bytes (+12,788); static SRAM is 95,412 bytes (+132), aligned end
`0x200174BC`, leaving 428,868 bytes in striped SRAM before runtime stacks.
Application load ends at `0x10099958`; ELF load segments exclude control keys and
both identity slots. Native release size is 8,033,104 bytes. The agent image remains exactly 4 MiB. No new external
firmware dependency or heap allocation was introduced; flash/identity logic is
host-testable through the shared no_std protocol. The existing upstream
`proc-macro-error2` future-incompatibility notice remains.

The native window was launched for layout inspection and the updated release
opened with the private profile. Independent power-cycle persistence, connected
editor interaction/invalid-input feedback, worst-case radio blackout/runtime stack,
USB shutdown fault injection, HID contention/physical Stop, long soak and
Linux/Windows acceptance remain unconfirmed. Populated/stale real agent metadata
and installer-stage acceptance are unchanged pending checks in the active plan.


USB control discoverability follow-up (2026-10-11): the user could not find the
previous state-dependent USB toggle below the status table. The editor now shows
separate, consistently labelled Enable USB and Disable USB buttons with USB state
and disabled-action guidance. They remain available independently of identity
capability, use the same bounded commands, and stay steady during background reads.
This changes native presentation only; no firmware update is required. The user
could not perform the interactive editor/power-cycle check, so those acceptance
items remain pending.


Follow-up checks passed: `cargo test -p companion-core -p pico-companion`
(37 tests), `cargo clippy -p pico-companion -p companion-core --all-targets -- -D warnings`,
`cargo run -p pico-companion -- --mock --self-test`,
`cargo build -p pico-companion --release`, `cargo fmt --all -- --check`, and
`git diff --check`. The actual mock window was captured with the development
screenshot feature and inspected to confirm both USB buttons are visible even
before connecting. The default release was rebuilt and reopened with the private
profile; connected interactive acceptance remains pending.


## Physical USB detach correction — 2026-10-11

The user confirmed the native name change worked (Yoyo / Xolo), but macOS still
listed the device while the companion reported USB disabled. Live IORegistry
also retained an active IOUSBHostDevice, so this was a firmware detach defect.
The pinned [Embassy RP 0.10.0 USB driver](https://github.com/embassy-rs/embassy/blob/embassy-rp-v0.10.0/embassy-rp/src/usb.rs)
has an empty `Bus::disable`. Previous BLE-flag/readback/enumeration checks did not
establish physical detach; those results should not be read as removal acceptance.

The USB task now stops its futures, calls UsbDevice disable to notify handlers,
then explicitly clears the device pull-up, masks controller interrupts and stops
the controller under core-0 interrupt exclusion. It waits ten milliseconds before
completing shutdown and releasing session buffers. The existing driver restores
registers on the next start. No additional dependency, allocation, protocol field,
flash reservation, VID/PID/serial or board pin changed.

The user supplied BOOTSEL for this correction. ELF load/verification completed
before `picotool reboot`; control keys and identity slots were preserved. The
saved Yoyo / Xolo identity was already present after boot, and authenticated
identity smoke succeeded. The first external observer was started before USB
had enumerated after reboot and its initial-device assertion failed; the smoke
itself restored the saved names. Repeating the targeted hardware check with the
observer started after enumeration produced complete live removal evidence:

```text
Yoyo / Xolo -> absent
absent -> Pico BLE test / Identity test 20261011
Pico BLE test / Identity test 20261011 -> absent -> temporary identity
Pico BLE test / Identity test 20261011 -> absent -> temporary identity
Pico BLE test / Identity test 20261011 -> absent -> Yoyo / Xolo
```

The observer polled `/usr/sbin/ioreg -p IOUSB -a -l` every 100 ms and selected
VID/PID 0x1209/0x0001 without printing unrelated devices. All four actual removals
were seen, followed by restored manufacturer/product. The companion smoke verified
save/readback, repeated stop/start, fresh authenticated reconnect and restoration
of the initial USB state. It now holds USB off for five seconds to permit external
removal inspection before its existing ten-second temporary-descriptor hold.
No HID text was sent. Physical detach and next-start descriptor application have
passed on this macOS/Pico combination. This does not establish other-platform
USB timing, independent power-cycle, fault-injected shutdown or runtime stack use.

Checks passed (36 portable firmware + 37 native/core tests):

```sh
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo test -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core --all-targets -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo build -p pico-companion --release
cargo run -p pico-companion -- --mock --self-test
cargo fmt --all -- --check
git diff --check
```

Deployment used `picotool load -u -v target/thumbv8m.main-none-eabihf/release/pico_rust -t elf`
then `picotool reboot`. Both identity smoke runs used
`target/release/pico-companion --profile <private-profile> --identity-self-test`.
The updated native release was reopened with the private profile afterwards.

Linked flash including initialized data is 4,823,704 bytes (+324 from the identity
slice); static SRAM is 95,416 bytes (+4). The aligned SRAM end remains
`0x200174BC`; application load ends at `0x10099A9C`. ELF segments still exclude
control keys and identity slots, and the agent image remains exactly 4 MiB.
The existing upstream future-incompatibility notice is unchanged.


## Encrypted BLE events and native filesystem browsing — 2026-10-11

Milestone 2 implements negotiated stream/filesystem v1 (Info flag 32, current
flags 60), an Events notification characteristic, native subscription, and
four-fragment cumulative credit. The stream has its own strictly increasing
sequence and disjoint nonce domain under the directional Noise key; finite
responses can overtake a partial stream without replay or nonce reuse. No USB
wire version, key provisioning, identity slot, KBD1 or HID ownership changed.
The native host-only futures-util dependency reuses the locked version.

Firmware owns one 2,070-byte ciphertext, one queued 2,048-byte directory page and
eight best-effort button events. Values stay 20 bytes at every negotiated MTU.
A send wait is bounded to 100 ms; a partial stream expires after five seconds
without GATT activity. Native directory/probe I/O is bounded to 30 seconds, with a
35-second client listing deadline. The browser caps accumulated entries at 512,
validates full pages/correlation/cursor progress/duplicate names, and follows the
USB target's path conventions rather than the control PC's. Paths exceeding the
512-byte limit remain visible as disabled entries. Consumed menu/Payloads/Stop
button gestures are excluded. Generic transfer sinks remain unavailable; this
milestone does not acknowledge bulk file data or introduce a native download.

The user explicitly supplied BOOTSEL for the corrected build. Deployment used
`picotool load -u -v target/thumbv8m.main-none-eabihf/release/pico_rust -t elf`,
verified completion, then `picotool reboot`. ELF segments exclude control keys
and identity slots. The existing private profile and saved Yoyo / Xolo identity
were preserved; no key/identity write, USB toggle, or HID effect was issued by
stream acceptance. An unchanged local host agent served a disposable fixture of
100 empty files, one hidden file and one folder; only directory metadata crossed
BLE. Test outputs do not contain keys or file contents.

Apple Silicon macOS / Pico evidence:

- Ordinary real BLE `--self-test` passed protected status/overview, commands during
  background refresh, release and fresh reconnect, with USB ready and agent present.
- Three complete stream/filesystem runs passed after the final-ACK correction.
  Three consecutive deterministic 2,048-byte probe records per run took roughly
  5.4–5.7 seconds (360–380 payload bytes/s). macOS negotiated ATT MTU 124; each
  notification value remained 20 bytes. These results do not establish larger
  fragments or a physically forced MTU 23 central.
- Real filesystem pagination returned 102 entries across 2,006/2,008/2,008/388-byte
  pages, with cursors 33/65/97/102. Hidden metadata was included, and a missing child
  path returned status 1 without losing control.
- The extended run cancelled both an unstarted request and a request after one
  second of activity, in 0.419/0.432 seconds. A fresh listing immediately after
  each cancel succeeded on the same authenticated connection, discarding the
  cancelled stream's obsolete ID. Post-browsing queue high-water was [4, 0, 1]
  (outstanding fragments, queued buttons, queued pages). No physical button was
  pressed during this measurement.
- Each run released/disconnected, authenticated a fresh connection and received
  another maximum-size stream record successfully. No request/HID replay occurred.

The initial maximum-record failure was a sender bug: final ACK cleared length,
but idle polling attempted to fragment the empty record with the previous offset.
`next_fragment()` now returns None for an empty record; the regression test polls
an idle sender after its final receipt, starts another record and retries the old
receipt. One early run of the corrected firmware later disconnected on its second
filesystem page without a diagnostic cause; the following three complete runs
passed. Keep longer soak/forced-disconnect acceptance open; this isolated event
has not been attributed to a confirmed cause.

Passed validation: 15 protocol, 8 session/crypto, 35 companion-core, 9 native
companion and 39 portable firmware tests (106 total, excluding doc tests):

```sh
cargo test -p ble-protocol -p ble-session -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p ble-session --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

Hardware commands (private profile contents were never printed):

```sh
target/release/pico-companion --profile <private-profile> --self-test
target/release/pico-companion --profile <private-profile> --stream-self-test --test-directory <disposable-directory>
```

Tests cover exact maximum pages, truncation and hostile fields, strict padding,
stream/command lane confusion, independent nonce/replay counters, credit pressure,
idempotent final receipts, relay saturation/USB ID remapping, stale/duplicate/
superseded pages, disconnect/timeout without replay, native cancellation priority,
Windows/UNC/extended paths, Unix backslash names and display-routing exclusions.

Linked allocated flash including initialized data is 4,834,036 bytes (+10,332
from the detach correction); static SRAM sections total 99,800 bytes (+4,384),
with aligned SRAM end 0x200185DC. This leaves 424,484 bytes of striped SRAM before
runtime stacks/allocations. Application flash load ends at 0x1009C2FC; the 4-MiB
agent image and reserved configuration/key regions are unchanged. Native release
size is 8,100,592 bytes. Static sizes do not measure worst-case runtime stack
headroom. No firmware dependency or heap allocation was added. The existing
upstream proc-macro-error2 future-incompatibility notice remains.

The actual mock and updated BLE native windows were launched. An attempted window
capture was unavailable, so visual rendering and interactive navigation are not
claimed as accepted. Home/Root/Up, breadcrumbs, hidden toggling, folder double-click,
selection/Copy path, Load more and error recovery need a connected user check.
Physical button delivery and Y Stop under stream/HID load, runtime stack headroom,
slow/unresponsive agent and USB-loss recovery, repeated lease cycles/long soak,
forced MTU 23/larger-fragment measurements, and Windows/Linux acceptance remain
open in NATIVE_COMPANION_BLE_PLAN.md. Native Python scripting is the next
implementation milestone; downloads require separate end-to-end backpressure and
USB ACK timeout measurements.


## Native Python milestone — 2026-10-12

Native Python implementation is complete; physical typing and release acceptance
remain open. The same executable starts a restricted RustPython 0.6.0 child lazily,
with cleared environment, bounded/versioned/run-scoped IPC, 128-MiB accounted Rust
heap, hard startup/step deadlines and kill/reap on Stop or shutdown. The recovered
prelude supports generator send/throw, all former keyboard effects, local timers,
button/capability waits and availability checks. General KBD1 remains strictly
validated and uses the existing authenticated BLE/script-v1 firmware protocol.
There is no firmware change or flash for this milestone.

Passed validation: 37 companion-core, 9 native companion, 2 actual-process
integration and 12 runtime tests (60), plus 4 layout, 9 executor, 7 build-support
and 2 script-protocol tests (22; 82 total, excluding doc tests):

```sh
cargo test -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --python-self-test
target/release/pico-companion --python-self-test
target/release/pico-companion --mock --self-test
cargo build -p pico-companion --release
cargo test -p keyboard-core --features 'std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de'
cargo test -p firmware-exec -p build-support -p script-protocol
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

Actual-process acceptance covered five mock keyboard completions, button delivery
through client/IPC, retained generator state across disconnect/throw/reconnect
without replay, a non-yielding subsequent step killed after 500 ms, denied imports,
a failed 256-MiB Python allocation, repeated allocation pressure and recovery,
Stop during non-yielding startup followed by a fresh child, and scoped HID
cancellation. Malformed lengths, unknown versions, bad command order, invalid
keyboard/output bounds and extra JSON fields are rejected. Repeated allocation
pressure recovered at 127.72 MiB accounted peak; small release runs used roughly
2.81 MiB. A small waiting helper measured approximately 11 ms to first yield and
20 MiB RSS on Apple Silicon macOS. These are individual measurements, not worst-case
RSS/stack guarantees. Native release size is roughly 25 MB (previously 8.1 MB).
The existing embedded proc-macro-error2 future-incompatibility notice remains.

Real Pico acceptance passed with the existing milestone-2 firmware:

```sh
cargo run -p pico-companion -- --profile <private-profile> --python-ble-self-test
```

It discovered/connected/authenticated/acquired, evaluated require_usb() and
submitted three empty KBD1 effects through the actual generator/client/BLE/Pico
executor with correlated completions. No characters were typed; empty effects
still perform key-release cleanup. USB was not toggled, saved Yoyo/Xolo identity
was preserved, and no profile contents were printed or passed to the child.
The board was initially still in BOOTSEL, causing discovery to time out;
`picotool reboot` started its existing firmware, after which the test passed.

The actual updated native window was launched with the private profile. Visual
rendering, typed text/chords, Stop during delay/key-down, final key release,
physical button/Y Stop/local-preset contention and USB/agent transitions still
need user/hardware acceptance. Longer soak and Linux/Windows process/BLE checks
remain open in NATIVE_COMPANION_BLE_PLAN.md. The child restricts Python host
capabilities and contains crashes/resource exhaustion; it is not an OS sandbox
against native VM vulnerabilities. The 128-MiB limit counts Rust allocations,
not total RSS/address space. See SCRIPTING.md and THREAT_MODEL.md.


## 2026-10-12: encrypted native downloads

Implemented transfer-v2 host-session/default/source controls, bounded native
queue/history and disk-backed receive/verify/save through the acquired BLE v3
connection. Download-v1 is advertised by Info flag 64 and uses encrypted inner
command 9, admission code 13 and stream kind 4. The BLE-only cancellation barrier
is tag 0 with no body; USB TLV tags/transfer-v2 formats are unchanged. Update the
companion and firmware together: the previous companion rejects the new Info
flag. New companions still decode the earlier v3 firmware flags. The updated
host agent starts with one chunk credit and a 60-second ACK deadline; firmware
withholds its one-credit USB ACK until native record validation/disk work and
final BLE credit. Stream inactivity is 30 seconds.

One firmware record (2,049 bytes), one owned native disk task, one retained ATT
write and one owned two-fragment ACK bound active buffering; the encrypted stream
maximum is 2,071 bytes, up one byte. Queue/history bounds are four/eight and the
provisional per-file limit is 1 MiB. Plaintext stages in an anonymous file, then
passes length/count/SHA-256 verification and a bounded copy/readback hash before
no-overwrite persistence and an encrypted receipt. Host filenames are sanitized
display metadata, never local paths; received content is not opened or executed.
See DOWNLOADS.md and THREAT_MODEL.md for trusted local-directory, source bootstrap,
plaintext-at-rest, final-save cancellation and platform durability boundaries.
No PSRAM pool, allocator, flash layout, provisioning key, pins or saved Yoyo/Xolo
USB identity was changed.

Software validation passed:

```sh
cargo test -p ble-protocol -p ble-session -p transfer-crypto -p transfer-protocol -p python-runtime -p companion-core -p pico-companion -p host-agent
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo clippy -p host-agent --all-targets -- -D warnings
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
cargo build -p pico-companion --release
target/release/pico-companion --mock --self-test
target/release/pico-companion --python-self-test
scripts/build-host-agent
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

The affected suites passed 182 non-doc tests in total: BLE protocol 16, session 8,
companion core 39, host unit 25 plus macOS PTY 6, native companion 25 plus actual
process integration 2, Python runtime 12, transfer crypto 3, transfer protocol 6,
and pure firmware 40. Coverage includes maximum/truncated/malformed records,
AEAD tampering, strict chunk indices/lengths (including u32::MAX), manifest/close
size/count/hash, exact retry idempotency, staging corruption before final copy,
no-overwrite destination races, queue/receipt ordering, source transfer-ID reuse,
stale session errors, write failure and cancellation cleanup. Retained disk, ATT
write and ACK tasks survive cancellation of their radio wait. Two consecutive
missing-source requests return bounded generic session errors and leave the host
worker running. Clippy and release builds passed; the existing embedded
proc-macro-error2 future-incompatibility notice remains.

The final flashed ELF allocates 104,056 bytes of SRAM and 4,839,624 bytes of flash
sections plus 3,612 bytes of initialized data in flash (4,843,236 bytes total,
including the four-MiB internal agent image). Its load segments do not overlap the
two provisioning slots at 0x10BFC000..0x10BFE000. These are linked static sizes,
not runtime stack headroom measurements.

Hardware testing was explicitly authorized. The user supplied BOOTSEL; three
ELF deployments were loaded, verified and rebooted using:

```sh
picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/pico_rust
```

The final firmware includes between-record relay-state cleanup on Cancel and
finite rejection/disconnect reason logging without keys, credentials or contents.
A local updated agent served disposable ignored fixtures over USB CDC. Headless
checks used the existing private profile (never printed) and explicit fresh local
filenames:

```sh
target/release/pico-companion --profile <private-profile> --download-self-test \
  --test-file <disposable-host-file> --test-destination <new-local-filename>
# Empty/default-path check: add --test-default-path.
# Cancellation checks: add --test-cancel-after-ms 3000 or 15000.
```

Preliminary failures are kept separate from acceptance: the first run let a new
control overtake the final session-record ACK, occupying the one-slot bridge.
Flushing ACKs before commands allowed an 8,195-byte byte-exact save with zero
retries (44.852 seconds), but that early harness run then misreported idle session
cleanup as a failure; it is receive/save evidence, not a complete CLI pass.
After the second flash, authentication/control initially disconnected, including
a verified handshake followed by a lost encrypted read. Cold restart alone did
not resolve it; the provisioning key was preserved. Firmware diagnostics later
identified invalid upload framing. Retaining only one ATT write passed three
cancellation cycles but failed the fourth. The final companion also owns the
whole sealed two-fragment ACK across refresh preemption; it never reseals that ACK
under another token. This is the tested ordering correction. Do not attribute
every earlier authentication interruption to an unobserved radio cause.

On the final companion, five consecutive cancellation/reconnect cycles passed
with exit 0, fresh encrypted host sessions and no local destination or named
staging leftovers. Two cycles cancelled after 2,002 accepted bytes; the others
cancelled during the first chunk. The updated firmware released USB relay waits
and accepted subsequent sessions beyond its four-slot relay capacity. The
empty/default-path save passed on the retained-write build with exit 0 and a
source-side encrypted completion receipt.

The final owned-ACK build saved the full 8,195-byte fixture in 39.797 seconds
with zero retries and exit 0 after control release/disconnect. Local byte and
SHA-256 comparisons matched the source, and the agent logged an encrypted file
transfer completion, confirming receipt delivery. A missing-source request
returned the bounded generic host failure with no destination; the next fresh
connection then saved an empty default-path file in 2.808 seconds with exit 0 and
an encrypted host receipt. No named staging or cancellation destinations remained.

After the user independently power-cycled the final firmware, the companion
authenticated/acquired with the unchanged profile, cancelled a file, drained the
old stream/barrier, established a fresh host session on the same BLE connection,
received a second manifest and cancelled again. Exit was 0, both source sessions
established and no destination/named staging remained. The first same-link attempt
had used the general four-second backend deadline while the client allowed 30
seconds; draining the old maximum record exceeded four seconds. Download commands
now share the core's 30-second deadline, while priority Cancel retains its shorter
four-second radio guard. The accepted command adds
`--test-cancel-after-ms 3000 --test-restart-after-cancel` to the headless check above.
This is one cold-start result, not repeated reboot/soak acceptance.

The actual native window is launched with the private profile for interactive
review. Headless checks establish transport/crypto/disk behavior; they do not
establish connected widget input/rendering acceptance. Release binary size is
approximately 25 MB. Full/slow/unwritable disk, source mutation/disappearance,
forced USB/agent loss, abrupt app exit/final-save races, saturated HID/physical Stop, maximum file
size/counter lifetime, independent cold-start soak, forced MTU 23 and real
Windows/Linux BLE/filesystem checks remain open in NATIVE_COMPANION_BLE_PLAN.md.


## 2026-10-19: native profile recovery, destination picker and macOS bundle

Implemented the next native companion slice without changing BLE/USB formats or
firmware. The window can import/select a private factory profile, retain the
last selection in a bounded non-secret catalog, and restore its key from macOS
Keychain on later launches. `--profile PATH` remains a session-only override.
Import is not provisioning/rotation: it never flashes, deletes the source or
replaces an existing different key. Windows/Linux native stores remain pending.

The catalog is at most 8 KiB/16 profiles, with bounded validated IDs/names and no
key/source-path fields. An OS-released exclusive lock serializes updates; keys
are inserted before same-directory catalog commit. Failure rolls back only newly
inserted credentials, never preexisting backups. Denied/locked/missing storage
and malformed catalogs fail explicitly, without automatic plaintext fallback.
Crashes may leave an unlisted Keychain item; reimporting its original profile
restores catalog membership. Keep external trusted provisioning backups.

Profile activation is a bounded backend intent requiring disconnected/idle state
and stopped Python. It replaces/drains the radio connector with a newer epoch,
clearing old selection/queued work before scanning again. Deliberate **Reconnect
previous Pico** scans for the previous device ID, authenticates fresh and leaves
control released. It never picks a different discovered device or replays work.
Initial device selection is now explicit. The existing BLE self-test now uses
this reconnect action and checks authority is not silently reacquired.

**Choose…** starts a native save dialog without blocking egui or BLE. The worker
validates its result, refuses existing files/symlinks, and returns epoch/state/draft
correlation. A stale result cannot replace a newer draft or connection. Selecting
a filename does not create/queue a file; final receiver no-overwrite verification
is unchanged. A pending native dialog/credential prompt cannot defer scoped
backend shutdown; the local worker is not joined while blocked.

Validation commands/results:

```sh
cargo fmt --all -- --check
git diff --check
cargo test -p ble-protocol -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --python-self-test
cargo build -p pico-companion --release
cargo test -p pico-companion profiles::tests
cargo test -p pico-companion desktop::tests
scripts/build-companion
./target/release/pico-companion --credential-store-self-test
'./target/Pico Companion.app/Contents/MacOS/pico-companion' --credential-store-self-test
'./target/Pico Companion.app/Contents/MacOS/pico-companion' --python-self-test
codesign --verify --deep --strict 'target/Pico Companion.app'
plutil -lint 'target/Pico Companion.app/Contents/Info.plist'
```

The affected suite passed **110 non-doc tests**: BLE protocol 16, companion core
43, native app 37, actual Python process integration 2 and runtime 12. Added
coverage includes reconnect without acquire/replay, missing previous devices,
cancelled/late scans, live-profile-change rejection during an owned job, epoch
replacement/stale UI commands, catalog corruption/bounds/locking, credential
denial/missing/conflicting keys, idempotent import, new-key rollback versus
preexisting-key preservation, destination races/symlinks/stale drafts, and
non-blocking shutdown with an outstanding native dialog future. The final
profile-only suite passed all 8 tests after lock-error/platform guard refinement.
The three focused desktop tests also passed after checking disconnect state as
well as epoch/draft: Disconnect itself does not advance the client epoch.
Clippy, formatting, release builds and mock/actual-process self-tests passed.

Live native credential-store checks passed from the release executable and
bundled executable: create one random disposable account, read/compare its
versioned record, reject overwrite, delete, then confirm absence. No real profile
was imported or modified by these checks; no credential bytes were printed or
passed as arguments. Fault/rollback tests use a fake store and disposable files.

The builder produced `target/Pico Companion.app` with Bluetooth usage strings,
a verified local ad-hoc signature and the same executable for Python worker mode.
Packaged actual-process Python acceptance passed, including deadlines, denied
imports, memory pressure, hard Stop and scoped mock effects. The bundle contains
only its executable, Info.plist and signature resources: no profile, key, external
interpreter or host-agent artifact. Executable size is approximately 25.1 MB
(25,128,800 bytes on this Apple Silicon build). This is a local native-architecture
artifact, not a notarized/distribution-signed installer or accepted upgrade.

The actual updated native window was launched, including the packaged build.
The development GL capture returned black, and a targeted OS window capture
could not create an image, so visual rendering is not accepted from those checks.
Native import/select/relaunch, file-picker click paths, permission prompts,
keyboard/focus behavior and connected rendering still need interactive acceptance.
No live Pico reconnect, HID, USB/agent or firmware test was run in this milestone;
no USB Pico was enumerated during validation and no firmware flash was needed.
Linux/Windows credential/dialog/BLE acceptance and macOS signed installation/upgrade
checks remain in the plans.

Native integration references: [RFD async/threading guidance](https://docs.rs/rfd/0.17.2/rfd/#macos-non-windowed-applications-async-and-threading),
[Security Framework password APIs](https://docs.rs/security-framework/3.7.0/security_framework/passwords/index.html),
and [Apple protected-resource descriptions](https://developer.apple.com/documentation/bundleresources/protected-resources).


## Native companion shell/theme milestone — 2026-10-24

Implemented task workspaces (Overview, Devices, Keyboard, Scripts, Files,
Transfers, Settings, Help and Activity), persistent connection/control and
owned-job panels, compact task selector, optional resizable diagnostics drawer
and launch-local System/Light/Dark choices. Drafts remain in app state across
navigation; core epoch and authoritative command admission are unchanged.
Stop routes active Python through its supervisor and manual/unrelated keyboard
work through scoped Cancel; download cancellation is separate. Keyboard lowering
is cached by text/layout/delay and submission revalidates in the client.
No firmware, protocol, flash layout, packet capacity or dependency changed.

Validation commands:

```sh
cargo fmt --all -- --check
cargo test -p ble-protocol -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --python-self-test
scripts/build-companion
```

The targeted suites pass 115 tests: BLE protocol 16, core 43, companion 42,
actual Python process integration 2 and Python runtime 12. New coverage includes
readiness/admission agreement across connection/control/USB/pending/background
refresh, Stop ownership, validation invalidation, light/dark token contrast and
painted Stop visibility across all nine workspaces at 1120 × 760, 820 × 790,
480 × 520 and 200% zoom. Navigation fixtures retain an unsent draft and active
job. Token checks cover normal text on three surfaces at 4.5:1, field boundaries
at 3:1 and selected white text at 4.5:1. They do not measure every actual widget,
hover/disabled/focus state or prove full-content fit at those sizes.

Mock lifecycle and actual Python-child containment/Stop checks pass. The local
macOS app bundle builds and its ad-hoc signature verifies. The packaged native
window was launched in mock mode; macOS reports a 1120 × 792 window including
the native title bar. Targeted OS capture failed with “could not create image
from window”, so no captured visual acceptance is claimed. Interactive visual,
keyboard/focus, VoiceOver, connected Stop and platform checks remain open.
The user reported the Pico in BOOTSEL; no firmware flash or hardware effect was
needed or performed for this UI milestone.


## Discovery and execution readiness follow-up — 2026-10-24

User acceptance of the shell found that selecting a device before Connect was
not apparent. After connecting, the demo was started with control released and
faulted at `require_usb()`. These are recorded usability defects; the earlier
connected/Stop acceptance was not established by that attempt.

Implemented explicit radio selection rows, a named selection beside Connect,
inline disabled explanations and selected/connecting/connected toolbar labels.
Devices now presents Choose → Connect → Acquire. Overview and Scripts share
control/USB readiness with explicit Open Devices/Acquire control/Enable USB actions.
The unchanged built-in USB typing demo checks readiness before Start and
Ctrl/Cmd+Enter; general Python scripts can still start offline and wait for events.
UI source matching is only a declared-demo convenience, never a permission check
or a parser for arbitrary Python. Backend/interpreter effect checks remain final.
Known script faults show recovery text before collapsed bounded technical details;
traceback classification never changes connection/control state or executes work.
No device selection, connection, acquisition, execution or effect replay is automatic.

Validation (all passed):

```sh
cargo fmt --all -- --check
cargo test -p ble-protocol -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --python-self-test
scripts/build-companion
```

120 tests pass: BLE protocol 16, core 43, companion 47, actual-process integration
2 and Python runtime 12. New egui interaction coverage sends pointer events into
the real mock backend: disabled Connect without a choice, radio selection,
Connect, blocked demo Start/Command+Enter without acquired control, Acquire,
delayed text, navigation to Files and persistent Stop through cancellation.
Presentation tests cover current/stale choices in one/multiple-device lists,
background-refresh stability, missing profile, released control, USB disabled/not
ready, demo admission versus custom offline scripts, owned-run restart and
recovery copy for known/unknown errors. Existing layout/Stop geometry, client
correlation/cancellation and production Python child suites continue to pass.

The local macOS release app bundle builds and its ad-hoc signature verifies.
A fresh packaged mock window was opened for interactive acceptance; older windows
retain the earlier executable. Human connection-flow, keyboard/focus, Python demo
Stop, real BLE/HID and other-platform acceptance remain pending unless recorded
below. No new visual capture result or hardware acceptance is inferred from tests.
Firmware/protocol/dependencies/capacities are unchanged; no hardware flash was needed.


## Companion visual polish — 2026-10-24

Implemented the screenshot review as one UI pass: neutral canvas/chrome/cards in
both themes, consistent padding and smaller section headings, filled primary
buttons, quiet secondary buttons with hover/focus treatment, red active Stop,
text-labelled control/USB badges, full-width left-aligned navigation and radio
rows. A compact toolbar names the device once and keeps Release/Disconnect in
Device ▾. One fixed footer contains Stop, job state and Activity; download cancel
remains separate. Connected Devices replaces completed discovery/connect steps
with a summary, expandable device ID, Change device and current readiness.
Interpreter metrics move under Run details. Identity/path forms stack or reserve
picker/action width, filesystem tables can scroll horizontally, and keyboard
layout labels are friendly while actual IDs/lowering/limits remain unchanged.

Validation (all passed):

```sh
cargo fmt --all -- --check
cargo test -p ble-protocol -p python-runtime -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol -p python-runtime --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo run -p pico-companion -- --python-self-test
scripts/build-companion
```

121 tests pass: BLE protocol 16, core 43, companion 48, actual Python process
integration 2 and Python runtime 12. The connected/released-control fixture
checks toolbar/footer each stay at most 64 logical points at 1120 × 760 and
completed/inapplicable actions are absent. The real egui pointer/mock-backend
flow additionally exercises Release control and Disconnect through Device ▾.
Existing Stop geometry covers all workspaces, desktop sizes and 200% zoom.
Token contrast checks now include canvas/chrome/card/input/raised surfaces and
success text as well as existing normal/accent/error/warning pairs. These do not
establish every actual widget/disabled/hover/focus state or full-content fit.

The release bundle builds and its local ad-hoc signature verifies. A fresh
packaged mock instance was opened for visual review (1120 × 792 including title
bar). Targeted OS capture again failed with “could not create image from window”.
Native visual, keyboard-only,
VoiceOver, real BLE/HID/USB, multi-monitor and other-platform acceptance remain
open. The user's supplied screenshots describe the earlier build; no new captured
visual acceptance is claimed. No firmware, wire format, authentication, capacity,
dependency or asset-generation change and no device flash were needed.


## Sidebar current-page indicator correction — 2026-10-24

The user reported that the polished sidebar did not mark the active page. Its
borderless Button setting suppressed egui's selected background. Restored selected
frame painting while keeping inactive rows quiet, and added a persistent left
accent marker. Existing outer keyboard-focus rings remain separate from selection;
native selected-state accessibility metadata is retained.

The existing layout fixture now checks the actually painted selected background
without pointer interaction across every workspace in light/dark desktop layouts,
as well as its earlier Stop geometry checks. Validation commands are unchanged
from the visual-polish section above: formatting, the 121-test native matrix,
Clippy, mock lifecycle, actual Python process checks and local release packaging.
No firmware/protocol/dependency changes or device flash are involved. A fresh
mock window is opened after rebuilding; native visual and assistive-technology
acceptance remain pending.

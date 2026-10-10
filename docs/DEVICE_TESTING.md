# USB device test suite

`scripts/device-test` runs the hardware checks that are safe without joining the Pico Wi-Fi network. The suite is opt-in and is not part of normal `cargo test` because it requires exclusive access to one physical board.

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


## Read-only BLE spike

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
  stated these should not gate further BLE work; WLAN remains enabled for now.
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

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
scripts/device-test --list --skip-msc
scripts/device-test --port /dev/cu.usbmodem12302
scripts/device-test --skip-msc
scripts/device-test --skip-host-agent
scripts/device-test --usb-throughput-benchmark --benchmark-mib 4 --skip-host-agent --skip-msc
scripts/device-test --usb-raw-throughput-benchmark --benchmark-mib 4 --skip-host-agent --skip-msc
scripts/device-test --flash --elf target/thumbv8m.main-none-eabihf/release/pico_rust
```

Run the hardware-independent harness tests directly:

```sh
cargo test -p device-test
cargo clippy -p device-test --all-targets -- -D warnings
cargo test -p host-agent
cargo clippy -p host-agent --all-targets -- -D warnings
```

Use `--vid`, `--pid`, or `--msc-label` only when the corresponding firmware descriptor/build setting was deliberately changed. `--port` is validated against the selected VID/PID by the raw phase before it is opened by the host agent. The host-agent phase performs two keepalive round-trips at the production 10-second cadence by default; `--host-keepalives`, `--host-interval-ms`, and `--host-timeout-ms` provide bounded diagnostic overrides.

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

On macOS the wrapper then checks, without attempting a write, that both the media and mounted `PICO_AGENT` volume are reported read-only, that it is an exact 4 MiB USB FAT16 device, and that `/README.TXT`, `/MAC`, `/WIN`, `/LINUX`, and the required `/MAC/HOSTAGNT` artifact exist. Linux read-only mount metadata is checked when the common auto-mount paths and `findmnt` are available. Unsupported or unlocatable mount layouts are reported as skipped, not silently passed.

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
behavior separately on US and DE input layouts. Current single-Mac research
results and remaining acceptance gates are recorded in the root CDC plan.

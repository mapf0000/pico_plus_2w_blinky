# AGENTS.md

This file applies to the entire repository. It is the working guide for automated agents and human contributors. Keep changes focused, preserve unrelated work in the tree, and prefer the smallest validation set that covers the affected targets.

## Project at a glance

This Rust workspace produces three cooperating pieces:

- RP2350B firmware for the Pimoroni Pico Plus 2 W.
- A native Rust/egui companion controlling the board over Bluetooth.
- A host-side serial agent that handles commands, filesystem browsing, credentials, and USB bulk-transfer protocol handling (native receiver deferred).

The primary hardware has 16 MiB flash, 8 MiB PSRAM and 520 KiB SRAM. Firmware
uses `no_std` and Embassy. WLAN, HTTP/WebSocket, frontend assets and the Python
Worker are removed. Bluetooth is mandatory in firmware; no browser tools are
needed for any build.

```text
Native egui companion <-- authenticated BLE GATT --> Firmware <-- USB CDC --> Host agent
                                                        +-- USB HID keyboard
                                                        +-- internal CDC agent image
```

The companion lowers bounded text to KBD1 using shared keyboard machinery.
Hardware presets share the same non-preempting executor, scoped cancellation and
physical Y Stop. BLE authentication requires physical numeric comparison. No
persistent bonds or native Python/file-transfer workflow exists yet.

Detailed references:

- `docs/ARCHITECTURE.md`: component ownership, startup, lifecycles, and data flow.
- `docs/PROTOCOL.md`: canonical BLE GATT and preserved USB TLV/transfer/filesystem formats.
- `docs/HARDWARE.md`: board wiring, memory map, USB/Bluetooth configuration, flashing, and recovery.

## Non-negotiable constraints

- Keep firmware compatible with `no_std`. Do not introduce `std`, unbounded host collections, blocking I/O, or heap assumptions into firmware code.
- Treat all firmware capacities as part of the design. Check `heapless` capacities, channel depths, BLE packet/fragment limits, TLV limits, flash layout, stack usage, and PSRAM/SRAM placement when increasing payloads or concurrency.
- Keep target-specific flags isolated. The root `.cargo/config.toml` scopes linker flags to `thumbv8m.main-none-eabihf`; the firmware config selects the Cortex-M target. Do not add a root default target.
- Preserve protocol compatibility across firmware, companion, and host agent. Protocol changes are never complete when only one endpoint compiles.
- Preserve backpressure in the file-transfer path. Firmware must never acknowledge chunks without a native receiver; the current unavailable relay rejects delivery.
- Do not hand-edit generated build output. Change its source or generator instead.
- Do not flash hardware as part of routine validation unless the task explicitly calls for device testing and hardware is available.
- Never log passwords, database credentials, transferred file contents, or other secrets. Credential environment variables exist for tests/headless use only.

## Repository map

### Firmware: `firmware/`

- `src/main.rs`: board initialization, CYW43 Bluetooth, task startup, and pin assignments.
- `src/ble.rs`, `src/ble_control.rs`: GATT, pairing, bounded upload, session-scoped HID completion.
- `src/usb/`: USB HID, CDC control/relay protocol, internal agent image, and USB supervision.
- `src/display/`: on-device pages, including standalone Payloads, input, rendering, and status views.
- `src/display_core/`: hardware-independent button routing, page models, bounded row scenes, and incremental rendering. `src/display/` owns GPIO/SPI/ADC and service adapters.
- `src/device_config.rs`: persistent flash-backed configuration. Its constants must agree with `memory.x`.
- `src/psram_pool.rs`: board-specific PSRAM initialization for diagnostics.
- `memory.x`: 16 MiB flash layout, including two persistent 4 KiB configuration slots.
- `build.rs`: delegates the build pipeline to `crates/build-support`.
- `cyw43-firmware/`: checked-in Wi-Fi/Bluetooth firmware blobs required by the embedded build.

The package is named `pico_rust`. Default features are `firmware` and `psram`; the selected Embassy RP feature is `rp235xb`.

### Host agent: `apps/host-agent/`

- `src/main.rs`: Tokio daemon and serial reconnection loop.
- `src/config.rs`: CLI parsing and logging setup.
- `src/transport.rs` and `src/tlv.rs`: serial selection, framing, resynchronization, and transport limits.
- `src/dispatch.rs`: incoming command routing.
- `src/file_transfer.rs`: chunked file sender and ACK/result handling.
- `src/filesystem.rs`: bounded, paginated directory listing protocol.
- `tests/e2e_mac.rs`: macOS pseudo-terminal end-to-end tests.

The host agent must remain portable unless code is explicitly target-gated. The macOS e2e suite is gated with `cfg(target_os = "macos")` and is skipped elsewhere.

### Native companion: `apps/companion/`

- `src/app.rs`: native egui rendering and controls.
- `src/backend.rs`: bounded UI commands, Tokio backend and shutdown.
- `src/smoke.rs`: deterministic headless mock lifecycle check.
- `src/ble.rs`: bounded native BLE worker, read-only discovery/status and cleanup.
- `crates/companion-core`: platform-independent client state, keyboard lowering,
  correlation, deadlines and mock transport.

The package is `pico-companion`. Choose `--mock` or `--ble`; BLE supports
read-only status against firmware built with `--features ble`. No BLE control
writes, authenticated pairing, or USB access are implemented. Native builds must remain independent of firmware asset
generation and browser tooling. Require pairing before control. Keep device work out of UI callbacks, bound
queues/diagnostics, and scope commands/results to a connection incarnation.

### Shared crates: `crates/`

- `ble-protocol`: fixed-size, `no_std` read-only BLE information/status codecs and UUIDs.
- `keyboard-core`: portable, language-neutral layouts, key parsing, lowering, and KBD1 encoding. It is `no_std` by default.
- `firmware-exec`: strict, two-pass `no_std` KBD1 validator/executor.
- `script-protocol`: versioned correlated companion-to-firmware effect envelopes.
- `ble-protocol`: `no_std` information/status/result values and MTU-23 command fragmentation.
- `bytecode-constants`: cross-target bytecode limits.
- `build-support`: typed keyboard-preset generation, linker setup, and internal FAT agent-image generation.

### Scripts and configuration

- `scripts/pico-run`: Cargo runner that flashes with `picotool`, waits for USB CDC, and streams logs.
- `scripts/build-host-agent`: release-builds a host agent and copies it to `apps/host-agent/artifacts/<target>/`.
- `scripts/fw-deploy-with-agent`: builds the local host agent, packages it, then builds/flashes firmware.
- `.cargo/config.toml`: target-scoped runner/linker flags and Cargo aliases.
- `flake.nix`: a Nix environment with Rust embedded support and `picotool`.

## Generated and ignored files

Do not commit or manually modify these outputs unless a task explicitly changes the artifact policy:

- `target/`
- `apps/host-agent/artifacts/`
- generated keyboard presets and `host-agent.img` under Cargo `OUT_DIR`

`Cargo.lock` is intentionally tracked for this application workspace. Include lockfile changes when dependency resolution genuinely changes, and avoid unrelated lockfile churn.

## Toolchain setup

From the repository root:

```sh
rustup target add thumbv8m.main-none-eabihf
```

Install `picotool` only for flashing. On macOS, `brew install picotool` is the usual route.

There is no pinned `rust-toolchain.toml`; use a current stable Rust toolchain capable of the workspace's Rust 2024 crates.

## Build and run commands

Run commands from the repository root unless noted otherwise.

### Firmware

Build without flashing:

```sh
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Build, flash, and tail USB logs:

```sh
cargo run -p pico_rust --release --target thumbv8m.main-none-eabihf
```

The equivalent Cargo alias is `cargo fw-deploy`. Running Cargo inside `firmware/` also selects the embedded target through `firmware/.cargo/config.toml`.

Build and package the local host agent before flashing:

```sh
scripts/fw-deploy-with-agent
```

Firmware compilation runs `firmware/build.rs` to install the linker script,
generate keyboard presets and embed the internal four-MiB FAT16 host-agent image.
It does not invoke Trunk, wasm-bindgen or RustPython.

### Native companion

```sh
cargo run -p pico-companion
cargo run -p pico-companion -- --mock
cargo run -p pico-companion -- --ble --self-test
```

### Host agent

Build or run on the current host:

```sh
cargo build -p host-agent
cargo run -p host-agent -- --help
```

Package a release binary for the detected/default target:

```sh
scripts/build-host-agent
```

Pass a target triple as the first argument to package another installed target. `HOST_AGENT_TARGET` overrides target detection in `scripts/fw-deploy-with-agent`.

## Validation matrix

Always format before handoff:

```sh
cargo fmt --all -- --check
```

Use targeted checks while iterating. Do not rely on bare `cargo test` at the workspace root: `default-members` points at firmware, whose normal target and build script are embedded-specific.

### Native companion changes

```sh
cargo test -p ble-protocol -p companion-core -p pico-companion
cargo clippy -p pico-companion -p companion-core -p ble-protocol --all-targets -- -D warnings
cargo run -p pico-companion -- --mock --self-test
cargo build -p pico-companion --release
```

Launch the actual native window for rendering/input changes. Mock tests do not
replace real BLE pairing, radio coexistence, hardware HID, or platform checks.
Also run the layout/executor suites when keyboard lowering changes.

### Host agent changes

```sh
cargo test -p host-agent
cargo clippy -p host-agent --all-targets -- -D warnings
```

On macOS, the test command includes the PTY-based e2e tests. Add unit tests beside protocol/config logic and extend e2e coverage when behavior crosses the daemon/serial boundary.

For secure file-transfer changes, also run `cargo test -p transfer-crypto -p transfer-protocol` and the affected embedded checks below. BLE, hostile-input, and hardware acceptance coverage is tracked in `docs/DEVICE_TESTING.md`; unit tests and embedded compilation do not replace those checks.

### Keyboard layout or bytecode changes

```sh
cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
cargo test -p firmware-exec -p build-support -p script-protocol
```

Compile the real embedded target. Changes to bytecode format/limits require
matching companion, keyboard-core, firmware-exec and script-protocol updates.

### Firmware changes

Display model, input, and rendering tests run on the host without firmware assets
or RP peripherals:

```sh
cargo test -p pico_rust --lib --no-default-features
cargo clippy -p pico_rust --lib --tests --no-default-features -- -D warnings
```

The build script skips embedded asset generation when the `firmware` feature is
disabled. Keep pure display modules free of generated presets and device globals.

Compile the real embedded release target:

```sh
cargo check -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Use `cargo build` instead of `cargo check` when validating linker layout, generated embedding, or final size. Hardware-dependent changes need an explicit board smoke test when hardware is available; document when that was not possible.

### Broad or cross-cutting changes

Run formatting, every affected targeted suite above, and an embedded release build. Prefer targeted Clippy commands because a single host-target workspace invocation is not valid for all `no_std` and embedded members.

## Protocol change checklist

USB CDC frames use a one-byte tag plus a four-byte little-endian payload length, with a maximum payload of 2048 bytes. File transfer and filesystem browsing add versioned little-endian payload formats on top.

When changing a tag, field, limit, status, event, or binary envelope:

1. Update the host-agent encoder/decoder and dispatch/transport tag allowlists.
2. Update firmware constants, parsers, command routing, relay state, and capacity assertions.
3. Update companion BLE encoding/decoding and state models when a new receiver is added.
4. Preserve explicit protocol versions or introduce a deliberate version bump and compatibility behavior.
5. Check the 2048-byte TLV maximum, the existing script kind prefix, BLE MTU/fragment limits, `TRANSFER_BINARY_MAX`, path/name bounds, and queue capacities.
6. Add malformed, boundary, round-trip, ordering, retry, and cancellation tests as relevant.
7. Verify that disconnects, duplicate opens, backpressure, aborts, and timeouts still terminate cleanly.

Do not silently reuse an existing tag or reinterpret a payload without versioning. Numeric values are currently repeated across endpoints, so search globally for every tag/status constant before editing.

## Firmware implementation guidance

- Prefer fixed-capacity `heapless` types and checked capacity handling. Avoid `unwrap`/`expect` for external input or capacity growth; reserve them for statically proven invariants and explain the invariant.
- Keep async tasks non-blocking. Use Embassy channels, signals, timers, and mutexes appropriate to the executor context.
- Global task state usually needs `StaticCell`, `ConstStaticCell`, an Embassy mutex/channel, or atomics with documented ordering. Avoid unsynchronized `static mut`.
- Validate all USB, BLE, flash, and configuration inputs before use. Parse little-endian fields explicitly and reject truncation, excess lengths, invalid UTF-8, and trailing data where the protocol requires exact frames.
- Keep user-facing status synchronized across the display and companion when adding device state.
- Changes to `memory.x`, `FLASH_CAPACITY`, persistent slots, PSRAM initialization, HTTP buffer sizes, task pools, or channel depths require an embedded release build and a memory/size review.
- Pin assignments and CYW43 configuration in `main.rs` are hardware-specific. Do not generalize or change them without explicit hardware scope.
- Use existing `log`/defmt pathways and keep high-frequency paths from flooding logs.

## Bluetooth implementation guidance

- One connection, one outstanding request/effect, one bounded receiver.
- Commands require authenticated encryption and physical confirmation of the current displayed code.
- Keep TrouBLE diagnostic logging disabled: upstream security logs include key material.
- Use writes with response fitting default ATT MTU 23; never assume negotiated MTU.
- Tokens are nonzero, monotonic and never reused in a connection. Incarnations must not wrap.
- Cancel/reset bypass ordinary host queues; firmware completion publication cannot block HID cleanup.
- Preserve strict script validation, local job independence and physical Y Stop.
- Pairing currently does not persist bonds; record real macOS pairing tests separately from mocks.

## Host-agent implementation guidance

- Keep the reconnect loop resilient: serial selection/open/dispatch failures should not terminate the daemon unless configuration is invalid or shutdown is intentional.
- Bound command output, file chunks, directory pages, path lengths, and queues. Never buffer a whole arbitrary file merely for convenience.
- Use structured `tracing`; include actionable context but omit credentials and file contents.
- Normalize paths carefully, preserve pagination determinism, distinguish files/directories/symlinks, and return protocol status errors rather than panicking.
- Keep platform-specific dialogs and PTY/file-descriptor code behind `cfg` gates. Ensure portable code still compiles on Linux and Windows when changing common modules.
- Test framing resynchronization and fragmented serial reads when transport logic changes.

## Keyboard-effect guidance

Keep keyboard-core `no_std` by default and layout features opt-in. Preserve the
1,024-character text, 4,096-byte KBD1 and one-outstanding-effect limits unless all
endpoints/docs change together. Hardware presets contain no interpreter.
Reservation lasts through key-release cleanup and result publication. Disconnect
cancels only its companion incarnation; old results cannot update a new connection.
A future Python interpreter requires an explicit supervised-process/isolation design.

## Build-system and dependency guidance

- Keep `firmware/build.rs` thin; put reusable build logic in `crates/build-support`.
- When adding build inputs, update rerun/fingerprint tracking so Cargo rebuilds when they should without rebuilding on every invocation.
- Avoid new dependencies in firmware unless they support `no_std` and their memory/code-size impact is justified. Prefer workspace sharing for constants and pure protocol logic when it truly prevents drift across targets.
- Do not run broad dependency updates for an unrelated change. Review `Cargo.lock` and embedded/native compatibility when dependencies do change.
- Keep dual licensing (`MIT OR Apache-2.0`) intact for new reusable crates and substantial copied code.

## Documentation and handoff

Update documentation in the same change when commands, paths, hardware assumptions, protocols, environment variables, UI workflows, Python effects, or keyboard behavior change. The root `README.md` is the operator overview; this file is the contributor/agent guide; `docs/SCRIPTING.md` is the current Python API and lifecycle reference; `docs/THREAT_MODEL.md` defines the security boundary. Keep wire formats in `docs/PROTOCOL.md`, component ownership in `docs/ARCHITECTURE.md`, hardware facts in `docs/HARDWARE.md`, and device validation coverage in `docs/DEVICE_TESTING.md`.

Before handoff:

1. Inspect `git diff` and `git status`; do not overwrite or restore unrelated user changes.
2. Confirm generated files and local logs are not accidentally staged.
3. Run the applicable validation matrix and report the exact commands and results.
4. Call out hardware/platform tests that were not possible.
5. Summarize protocol, memory, security, or compatibility implications for cross-cutting changes.

Prefer small, reviewable commits with imperative subjects. Do not create commits, push branches, or flash devices unless explicitly requested.

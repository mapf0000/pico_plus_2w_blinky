# AGENTS.md

This file applies to the entire repository. It is the working guide for automated agents and human contributors. Keep changes focused, preserve unrelated work in the tree, and prefer the smallest validation set that covers the affected targets.

## Project at a glance

This Rust workspace produces three cooperating pieces:

- RP2350B firmware for the Pimoroni Pico Plus 2 W.
- A Yew/WebAssembly frontend that is built by Trunk and embedded in the firmware.
- A host-side serial agent that handles commands, filesystem browsing, credentials, and host-to-browser file transfers.

The primary hardware has 16 MiB QSPI flash, 8 MiB PSRAM, and 520 KiB SRAM. Firmware runs without a standard library and uses Embassy async tasks. The device creates the `PicoEndpoint` Wi-Fi access point, serves its UI at `http://192.168.4.1/`, and communicates with the host agent over USB CDC.

The main data paths are:

```text
Browser (Yew) <-- WebSocket /ws --> Firmware <-- USB CDC TLV --> Host agent
                                      |
                                      +-- USB HID keyboard
                                      +-- USB mass-storage host-agent image
```

For keyboard scripts, DSL source is compiled to bounded bytecode, transported to the device, and executed as USB HID reports.

Detailed references:

- `docs/ARCHITECTURE.md`: component ownership, startup, lifecycles, and data flow.
- `docs/PROTOCOL.md`: canonical USB TLV, WebSocket, transfer, and filesystem formats.
- `docs/HARDWARE.md`: board wiring, memory map, USB/Wi-Fi configuration, flashing, and recovery.

## Non-negotiable constraints

- Keep firmware compatible with `no_std`. Do not introduce `std`, unbounded host collections, blocking I/O, or heap assumptions into firmware code.
- Treat all firmware capacities as part of the design. Check `heapless` capacities, channel depths, WebSocket limits, TLV limits, flash layout, stack usage, and PSRAM/SRAM placement when increasing payloads or concurrency.
- Keep target-specific flags isolated. The root `.cargo/config.toml` scopes linker flags to `thumbv8m.main-none-eabihf`; the frontend config selects `wasm32-unknown-unknown`; the firmware config selects the Cortex-M target. Do not add a root default target.
- Preserve protocol compatibility across firmware, frontend, and host agent. Protocol changes are never complete when only one endpoint compiles.
- Preserve backpressure in the file-transfer path. Firmware deliberately waits for browser queue capacity before acknowledging USB chunks.
- Do not hand-edit generated build output. Change its source or generator instead.
- Do not flash hardware as part of routine validation unless the task explicitly calls for device testing and hardware is available.
- Never log passwords, database credentials, transferred file contents, or other secrets. Credential environment variables exist for tests/headless use only.

## Repository map

### Firmware: `firmware/`

- `src/main.rs`: board initialization, Wi-Fi AP, network stack, task startup, and pin assignments.
- `src/http/`: HTTP/WebSocket server, RPC routes, and embedded frontend serving.
- `src/usb/`: USB HID, CDC control/relay protocol, MSC image, and USB supervision.
- `src/display/`: on-device pages, input, rendering, and status views.
- `src/device_config.rs`: persistent flash-backed configuration. Its constants must agree with `memory.x`.
- `src/psram_pool.rs`: external-memory allocation for HTTP buffers.
- `memory.x`: 16 MiB flash layout, including two persistent 4 KiB configuration slots.
- `build.rs`: delegates the build pipeline to `crates/build-support`.
- `cyw43-firmware/`: checked-in Wi-Fi/Bluetooth firmware blobs required by the embedded build.

The package is named `pico_rust`. Default features are `firmware` and `psram`; the selected Embassy RP feature is `rp235xb`.

### Frontend: `apps/frontend/`

- `src/app.rs`: top-level Yew state, connection lifecycle, and section routing.
- `src/api.rs`: the single WebSocket connection, RPC correlation, reconnect behavior, and event routing.
- `src/ui/`: page and component rendering.
- `src/transfer/`: transfer protocol parsing, model/store, IndexedDB staging, verification, and download.
- `src/filesystem.rs`: host filesystem page decoding and browser state.
- `src/dsl.rs` and `src/scripts.rs`: DSL compilation and built-in script access.
- `ui/style.css` and `ui/idb.js`: static UI assets copied by Trunk.
- `Trunk.toml`: development proxy and watched paths.

The crate is always built for `wasm32-unknown-unknown`. It uses browser APIs and single-threaded Yew state; avoid blocking work in callbacks.

### Host agent: `apps/host-agent/`

- `src/main.rs`: Tokio daemon and serial reconnection loop.
- `src/config.rs`: CLI parsing and logging setup.
- `src/transport.rs` and `src/tlv.rs`: serial selection, framing, resynchronization, and transport limits.
- `src/dispatch.rs`: incoming command routing.
- `src/file_transfer.rs`: chunked file sender and ACK/result handling.
- `src/filesystem.rs`: bounded, paginated directory listing protocol.
- `tests/e2e_mac.rs`: macOS pseudo-terminal end-to-end tests.

The host agent must remain portable unless code is explicitly target-gated. The macOS e2e suite is gated with `cfg(target_os = "macos")` and is skipped elsewhere.

### Shared crates: `crates/`

- `dsl/dsl-core`: portable parser, preprocessor, linker, layouts, lowering, and bytecode. It is `no_std` by default; `std` is opt-in for host conveniences.
- `dsl/dsl-wasm`: standalone wasm-bindgen adapter around `dsl-core`.
- `dsl/firmware-exec`: `no_std` streaming HID bytecode executor.
- `builtin-scripts`: built-in keyboard scripts shared by frontend/build support.
- `bytecode-constants`: cross-target bytecode limits.
- `build-support`: firmware build-time frontend compilation, asset embedding, payload generation, linker setup, and MSC image generation.

Read `crates/dsl/README.md` before changing DSL syntax, layouts, limits, or bytecode behavior.

### Scripts and configuration

- `scripts/pico-run`: Cargo runner that flashes with `picotool`, waits for USB CDC, and streams logs.
- `scripts/build-host-agent`: release-builds a host agent and copies it to `apps/host-agent/artifacts/<target>/`.
- `scripts/fw-deploy-with-agent`: builds the local host agent, packages it, then builds/flashes firmware.
- `.cargo/config.toml`: target-scoped runner/linker flags and Cargo aliases.
- `flake.nix`: a partial Nix environment with Rust embedded support and `picotool`; Trunk and the wasm target may still need to be installed.

## Generated and ignored files

Do not commit or manually modify these outputs unless a task explicitly changes the artifact policy:

- `target/`
- `apps/frontend/dist/`
- `apps/host-agent/artifacts/`
- generated `frontend_static.rs`, `payloads_gen.rs`, and `host-agent.img` under Cargo `OUT_DIR`

The top-level `frontend/dist/` directory is a separate tracked snapshot; do not confuse it with Trunk's ignored `apps/frontend/dist/` output or refresh it incidentally.

`Cargo.lock` is intentionally tracked for this application workspace. Include lockfile changes when dependency resolution genuinely changes, and avoid unrelated lockfile churn.

## Toolchain setup

From the repository root:

```sh
rustup target add thumbv8m.main-none-eabihf
rustup target add wasm32-unknown-unknown
cargo install trunk
```

Install `picotool` only for flashing. On macOS, `brew install picotool` is the usual route. Frontend browser tests also require `wasm-bindgen-test-runner` and a compatible WebDriver/browser setup.

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

Firmware compilation runs `firmware/build.rs`. It may invoke Trunk, compile built-in DSL scripts, construct the 8 MiB FAT16 host-agent image, and embed all results. A firmware build therefore needs the frontend/tooling inputs even when the Rust change is firmware-only.

### Frontend

Development server:

```sh
cd apps/frontend
trunk serve
```

The development server proxies `/ws` to `ws://192.168.4.1/ws`; a powered device on the Pico access point is needed for live RPC behavior.

Release bundle:

```sh
cd apps/frontend
trunk build --release
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

### Host agent changes

```sh
cargo test -p host-agent
cargo clippy -p host-agent --all-targets -- -D warnings
```

On macOS, the test command includes the PTY-based e2e tests. Add unit tests beside protocol/config logic and extend e2e coverage when behavior crosses the daemon/serial boundary.

### DSL, layout, or bytecode changes

Run the full layout matrix:

```sh
cargo test -p dsl-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
```

Also compile affected adapters/executors for their real targets. If bytecode format or limits change, update and test `dsl-core`, `dsl-wasm`, `firmware-exec`, `bytecode-constants`, built-in payload generation, and frontend decoding as applicable.

### Frontend changes

At minimum, compile the wasm tests:

```sh
cargo test -p frontend --target wasm32-unknown-unknown --no-run
```

The integration tests are configured with `run_in_browser`; execute them with the repository's available browser/WebDriver setup when behavior changes. Also run `trunk build --release` for asset, HTML, CSS, or build-pipeline changes.

### Firmware changes

Compile the real embedded release target:

```sh
cargo check -p pico_rust --release --target thumbv8m.main-none-eabihf
```

Use `cargo build` instead of `cargo check` when validating linker layout, generated embedding, or final size. Hardware-dependent changes need an explicit board smoke test when hardware is available; document when that was not possible.

### Broad or cross-cutting changes

Run formatting, every affected targeted suite above, and an embedded release build. Prefer targeted Clippy commands because a single host-target workspace invocation is not valid for all `no_std`, wasm, and embedded members.

## Protocol change checklist

USB CDC frames use a one-byte tag plus a four-byte little-endian payload length, with a maximum payload of 2048 bytes. File transfer and filesystem browsing add versioned little-endian payload formats on top.

When changing a tag, field, limit, status, event, or binary envelope:

1. Update the host-agent encoder/decoder and dispatch/transport tag allowlists.
2. Update firmware constants, parsers, command routing, relay state, and capacity assertions.
3. Update frontend WebSocket event routing, transfer/filesystem decoders, and state models.
4. Preserve explicit protocol versions or introduce a deliberate version bump and compatibility behavior.
5. Check the 2048-byte TLV maximum, the one-byte WebSocket binary kind prefix, `TRANSFER_BINARY_MAX`, path/name bounds, and queue capacities.
6. Add malformed, boundary, round-trip, ordering, retry, and cancellation tests as relevant.
7. Verify that disconnects, duplicate opens, backpressure, aborts, and timeouts still terminate cleanly.

Do not silently reuse an existing tag or reinterpret a payload without versioning. Numeric values are currently repeated across endpoints, so search globally for every tag/status constant before editing.

## Firmware implementation guidance

- Prefer fixed-capacity `heapless` types and checked capacity handling. Avoid `unwrap`/`expect` for external input or capacity growth; reserve them for statically proven invariants and explain the invariant.
- Keep async tasks non-blocking. Use Embassy channels, signals, timers, and mutexes appropriate to the executor context.
- Global task state usually needs `StaticCell`, `ConstStaticCell`, an Embassy mutex/channel, or atomics with documented ordering. Avoid unsynchronized `static mut`.
- Validate all USB, HTTP, WebSocket, flash, and configuration inputs before use. Parse little-endian fields explicitly and reject truncation, excess lengths, invalid UTF-8, and trailing data where the protocol requires exact frames.
- Keep user-facing status synchronized across the display and Web UI when adding device state.
- Changes to `memory.x`, `FLASH_CAPACITY`, persistent slots, PSRAM initialization, HTTP buffer sizes, task pools, or channel depths require an embedded release build and a memory/size review.
- Pin assignments and CYW43 configuration in `main.rs` are hardware-specific. Do not generalize or change them without explicit hardware scope.
- Use existing `log`/defmt pathways and keep high-frequency paths from flooding logs.

## Frontend implementation guidance

- Keep WebSocket ownership centralized in `api.rs`; do not open per-component sockets.
- Keep pure protocol/state logic separate from Yew rendering so it remains testable.
- Correlate RPC responses by request ID, cancel pending work on disconnect, and preserve reconnect behavior.
- Treat all device/host data as untrusted. Validate JSON variants, binary kinds, lengths, versions, CRC32, SHA-256, chunk ordering, and filesystem pagination.
- Revoke browser object URLs and release IndexedDB/download state on completion, failure, retry, or component teardown.
- Update `ui/style.css` with responsive and accessibility states when adding UI: labels, keyboard behavior, disabled/busy states, focus visibility, empty/error/loading states, and narrow layouts.
- Keep WebAssembly size in mind. Firmware build thresholds are controlled by `PICO_WASM_WARN_BYTES` (default 1,200,000) and optional `PICO_WASM_MAX_BYTES`.

## Host-agent implementation guidance

- Keep the reconnect loop resilient: serial selection/open/dispatch failures should not terminate the daemon unless configuration is invalid or shutdown is intentional.
- Bound command output, file chunks, directory pages, path lengths, and queues. Never buffer a whole arbitrary file merely for convenience.
- Use structured `tracing`; include actionable context but omit credentials and file contents.
- Normalize paths carefully, preserve pagination determinism, distinguish files/directories/symlinks, and return protocol status errors rather than panicking.
- Keep platform-specific dialogs and PTY/file-descriptor code behind `cfg` gates. Ensure portable code still compiles on Linux and Windows when changing common modules.
- Test framing resynchronization and fragmented serial reads when transport logic changes.

## DSL and generated payload guidance

- Keep `dsl-core` `no_std` by default and avoid breaking its opt-in layout feature model.
- The DSL is deliberately bounded and deterministic. Any syntax expansion must retain source-mapped diagnostics, recursion/expansion caps, line limits, operation limits, delay limits, and bytecode size limits.
- Every script requires a leading `layout(...)`; called scripts inherit the entry layout. Raw `tap`/`modtap` operations bypass text layout mapping.
- Add parser/preprocessor/linker/lowering tests at the narrowest layer and end-to-end compile/decode tests for externally visible behavior.
- Update `crates/dsl/README.md`, examples, built-in scripts, and adapters whenever user-visible DSL behavior changes.
- Generated built-in payloads come from `builtin-scripts` through `crates/build-support`; edit those sources, not `OUT_DIR/payloads_gen.rs`.

## Build-system and dependency guidance

- Keep `firmware/build.rs` thin; put reusable build logic in `crates/build-support`.
- When adding build inputs, update rerun/fingerprint tracking so Cargo and Trunk rebuild when they should without rebuilding on every invocation.
- Scrub or scope embedded target variables when spawning wasm builds. Embedded `RUSTFLAGS` must never leak into Trunk.
- Avoid new dependencies in firmware unless they support `no_std` and their memory/code-size impact is justified. Prefer workspace sharing for constants and pure protocol logic when it truly prevents drift across targets.
- Do not run broad dependency updates for an unrelated change. Review `Cargo.lock` and embedded/wasm compatibility when dependencies do change.
- Keep dual licensing (`MIT OR Apache-2.0`) intact for new reusable crates and substantial copied code.

## Documentation and handoff

Update documentation in the same change when commands, paths, hardware assumptions, protocols, environment variables, UI workflows, or DSL behavior change. The root `README.md` is the operator overview; this file is the contributor/agent guide; `apps/frontend/README.md` and `crates/dsl/README.md` cover their subsystems.

Before handoff:

1. Inspect `git diff` and `git status`; do not overwrite or restore unrelated user changes.
2. Confirm generated files and local logs are not accidentally staged.
3. Run the applicable validation matrix and report the exact commands and results.
4. Call out hardware/browser/platform tests that were not possible.
5. Summarize protocol, memory, security, or compatibility implications for cross-cutting changes.

Prefer small, reviewable commits with imperative subjects. Do not create commits, push branches, or flash devices unless explicitly requested.

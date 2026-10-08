# Dependency update: 2026-10-08

Validated with Rust and Cargo 1.99.0 on aarch64 macOS. Updated Rust dependency
requirements to current stable releases and refreshed the workspace lockfile.
The initial refresh changed 109 package versions, including picoserve 0.20.1,
leasehund 0.6.0, getrandom 0.4.3, Tokio 1.53.2, and wasm-bindgen 0.2.129.
The subsequent RustPython upgrade also replaces its compiler/parser family.

Installed wasm-bindgen CLI/test runner 0.2.129 to match the library. Trunk
0.21.14 was already the latest stable release.

RustPython is now at 0.6.0 with its Ruff family pinned to 0.16.5. The prior
native debug initialization failure is resolved by optimizing the VM in the
test profile, and the compressed Worker limit is raised to 4,750,000 bytes
within the existing flash budget. See [the upgrade investigation](RUSTPYTHON_PROCESS_PLAN.md).
Upstream constraints also retain older transitive dependencies, including
Snow's getrandom 0.3, whose browser feature must remain enabled.
Nix inputs were not updated or evaluated because Nix is not installed in the
validation environment. The untracked, obsolete `dsl/Cargo.lock` has no
associated crate and was left alone.

## Validation

All commands below passed. Commands ran from the repository root except the
Trunk build, which ran from `apps/frontend`.

```sh
cargo test -p host-agent -p transfer-crypto -p transfer-protocol -p script-protocol -p build-support -p device-test -p mock-pico -p keyboard-core --features 'keyboard-core/std keyboard-core/layout_win_en_gb keyboard-core/layout_win_pt_br keyboard-core/layout_win_de_de keyboard-core/layout_mac_en_gb keyboard-core/layout_mac_pt_br keyboard-core/layout_mac_de_de'
cargo test -p host-agent -p transfer-crypto
cargo test -p keyboard-core --features "std layout_win_en_gb layout_win_pt_br layout_win_de_de layout_mac_en_gb layout_mac_pt_br layout_mac_de_de"
cargo test -p python-worker -p build-support
GECKODRIVER=/tmp/pico-geckodriver/geckodriver MOZ_HEADLESS=1 WASM_BINDGEN_TEST_WEBDRIVER_JSON=/tmp/pico-python-webdriver.json cargo test -p python-worker --release --target wasm32-unknown-unknown
cargo clippy -p python-worker -p build-support --all-targets -- -D warnings
cargo clippy -p host-agent --all-targets -- -D warnings
cargo test -p frontend --target wasm32-unknown-unknown --no-run
env -u NO_COLOR trunk build --release
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

Host coverage includes 24 unit tests and all six macOS PTY end-to-end tests;
all 11 RustPython process tests passed natively and in headless Firefox. The
embedded build includes the release wasm Worker, frontend bindings, generated
embedding, and linker layout.
See [hardware measurements](HARDWARE.md) for flash and static SRAM headroom.
The Firefox run used geckodriver 0.37.1 downloaded to `/tmp` and a temporary
WebDriver capabilities file selecting
`/Applications/Firefox.app/Contents/MacOS/firefox` as `moz:firefoxOptions.binary`.
Frontend browser tests and Linux/Windows builds were not run. The dependency
`proc-macro-error2` 2.0.1 still produces Cargo's future-incompatibility notice.

The user supplied an RP2350 board in BOOTSEL for device validation. These
commands also passed:

```sh
picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/pico_rust
cargo build -p device-test
target/debug/device-test --wait-secs 45
```

Flash verification and reboot succeeded; both CDC interfaces enumerated, and
all 11 USB control smoke cases passed. The suite sends synthetic control and
encrypted envelopes, without host-file access or HID keyboard effects. The
Wi-Fi-served browser workflow and physical HID/disconnect tests were not run.

The picoserve adaptation changes writer trait bounds; protocol formats,
queue capacities, memory regions, and scripting limits remain unchanged.

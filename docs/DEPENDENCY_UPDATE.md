# Dependency update: 2026-10-08

Validated with Rust and Cargo 1.99.0 on aarch64 macOS. Updated Rust dependency
requirements to current stable releases and refreshed the workspace lockfile;
109 package versions changed. This includes picoserve 0.20.1, leasehund 0.6.0,
getrandom 0.4.3, Tokio 1.53.2, and wasm-bindgen 0.2.129.

Installed wasm-bindgen CLI/test runner 0.2.129 to match the library. Trunk
0.21.14 was already the latest stable release.

RustPython remains at 0.5.0 because 0.6.0 fails initialization tests and the
Worker size gate; see [the upgrade investigation](RUSTPYTHON_PROCESS_PLAN.md).
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
cargo test -p python-worker
cargo clippy -p host-agent --all-targets -- -D warnings
cargo test -p frontend --target wasm32-unknown-unknown --no-run
env -u NO_COLOR trunk build --release
cargo build -p pico_rust --release --target thumbv8m.main-none-eabihf
cargo fmt --all -- --check
git diff --check
```

Host coverage includes 24 unit tests and all six macOS PTY end-to-end tests;
all eight RustPython process tests passed. The embedded build includes the
release wasm Worker, frontend bindings, generated embedding, and linker layout.
See [hardware measurements](HARDWARE.md) for flash and static SRAM headroom.
No device was flashed and browser tests were not executed. Linux and Windows
builds were not run. The dependency `proc-macro-error2` 2.0.1 still produces
Cargo's future-incompatibility notice.

The picoserve adaptation changes writer trait bounds; protocol formats,
queue capacities, memory regions, and scripting limits remain unchanged.

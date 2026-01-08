# Frontend (Yew + WebAssembly)

This is a small Yew application compiled to WebAssembly.

Build options:

- Using Trunk (recommended):
  - Install: `cargo install trunk` and `rustup target add wasm32-unknown-unknown`
  - Dev server: `trunk serve --open` (runs a local server with hot reload)
  - Release build: `trunk build --release`

- Using cargo + wasm-bindgen directly:
  - `rustup target add wasm32-unknown-unknown`
  - `cargo build -p frontend --release --target wasm32-unknown-unknown`
  - Then run `wasm-bindgen` on the resulting `.wasm` to generate the JS glue, or use `wasm-pack`.

Once built, you can copy the generated static files into your firmware’s `assets/` folder and extend the HTTP server to serve them.

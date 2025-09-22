#![cfg_attr(not(test), no_std)]

// Re-export only the modules that are platform-agnostic so we can run
// unit tests on a host (macOS) with `cargo test --lib`.
pub mod host;

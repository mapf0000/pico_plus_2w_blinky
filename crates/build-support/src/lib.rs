//! Build-support helpers invoked from the workspace `build.rs`.
//!
//! Responsibilities (kept small and focused):
//! - Install the linker script (`memory.x`) into `OUT_DIR` and add it to the
//!   link search path for the firmware target.
//! - Ensure the Web UI (Yew) is built with Trunk to an isolated release
//!   directory under `OUT_DIR` when sources change, then copy a few
//!   stable-named assets and generate `frontend_static.rs` with `include_*`
//!   statements. Development output in `apps/frontend/dist/` is never embedded.
//! - Compile built-in keyboard payloads from DSL into bytecode for the display
//!   and emit `payloads_gen.rs` into `OUT_DIR`.
//! - Avoid leaking embedded-only flags into the wasm build by scrubbing
//!   environment variables when spawning Trunk.
//! - Provide simple size guards via env variables.

use anyhow::{Context, Result, bail};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use which::which;

/// Entry point called from the workspace `build.rs`.
///
/// Steps:
/// 1) Emit `rerun-if-*` hints for relevant env and files.
/// 2) Copy `memory.x` into `OUT_DIR` and expose it to the linker.
/// 3) Rebuild the Web UI with Trunk if sources changed, then embed assets.
pub fn run() -> Result<()> {
    // Re-run on env var changes (size guards + MSC label)
    cargo::rerun_if_env(&[
        env_consts::WARN_BYTES,
        env_consts::MAX_BYTES,
        env_consts::MSC_LABEL,
        env_consts::FIRMWARE_BUILD,
    ]);

    // Re-run when these files change (build-support itself lives in its own crate)
    cargo::rerun_if_changed("build.rs");

    let cfg = Config::from_env()?;
    let firmware_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into());
    let firmware_build = env::var(env_consts::FIRMWARE_BUILD)
        .unwrap_or_else(|_| format!("{firmware_version}-{}", cfg.profile));
    let firmware_build: String = firmware_build
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
        })
        .take(64)
        .collect();
    if firmware_build.is_empty() {
        bail!("PICO_FIRMWARE_BUILD must contain at least one build identifier character");
    }
    cargo::rustc_env(env_consts::FIRMWARE_BUILD, &firmware_build);

    // 1) Linker script
    linker::install_memory_x(&cfg)?;

    // 2) Frontend pipeline (always required; auto-staleness detection)
    frontend::register_reruns(&cfg);
    frontend::prepare(&cfg)?;
    payloads::register_reruns(&cfg);
    payloads::prepare(&cfg)?;
    msc_image::register_reruns(&cfg);
    msc_image::prepare(&cfg)?;

    Ok(())
}

/* ----------------------------- Config & Env ------------------------------ */

#[derive(Debug)]
/// Build-time configuration captured from Cargo environment.
struct Config {
    /// Cargo's per-build output directory.
    out_dir: PathBuf,
    /// The manifest dir of the firmware crate (contains `memory.x`).
    manifest_dir: PathBuf,
    /// Workspace root (repo root).
    repo_root: PathBuf,
    /// Path to the frontend crate.
    frontend_dir: PathBuf,
    /// Path to the DSL folder (dsl-core, dsl-wasm, firmware-exec).
    dsl_dir: PathBuf,
    /// Active profile (e.g., `debug` or `release`).
    profile: String,
    /// Active compilation target triple.
    target: String,
    /// Soft size threshold for the built WebAssembly (emits a warning).
    warn_bytes: u64,
    /// Hard size limit for the WebAssembly (fails the build when exceeded).
    max_bytes: Option<u64>,
}

/// Environment variable names used by the build.
mod env_consts {
    pub const WARN_BYTES: &str = "PICO_WASM_WARN_BYTES";
    pub const MAX_BYTES: &str = "PICO_WASM_MAX_BYTES";
    pub const MSC_LABEL: &str = "PICO_MSC_LABEL";
    pub const FIRMWARE_BUILD: &str = "PICO_FIRMWARE_BUILD";
}

impl Config {
    fn from_env() -> Result<Self> {
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").context("OUT_DIR missing")?);
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
        let repo_root = manifest_dir
            .parent()
            .context("workspace root not found (expected firmware crate at firmware/)")?
            .to_path_buf();
        let frontend_dir = repo_root.join("apps/frontend");
        let dsl_dir = repo_root.join("crates/dsl");
        let profile = env::var("PROFILE").unwrap_or_default();
        let target = env::var("TARGET").unwrap_or_default();

        let warn_bytes = env::var(env_consts::WARN_BYTES)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_200_000);
        let max_bytes = env::var(env_consts::MAX_BYTES)
            .ok()
            .and_then(|s| s.parse().ok());

        Ok(Self {
            out_dir,
            manifest_dir,
            repo_root,
            frontend_dir,
            dsl_dir,
            profile,
            target,
            warn_bytes,
            max_bytes,
        })
    }

    #[allow(dead_code)]
    /// True when building a release for an embedded `thumb*` target.
    fn is_embedded_release(&self) -> bool {
        self.profile == "release" && self.target.starts_with("thumb")
    }
}

/* -------------------------------- Cargo ---------------------------------- */

/// Minimal helpers to emit Cargo build script directives.
mod cargo {
    use std::path::Path;

    /// Emit a `rustc-link-search` directive for the given directory.
    pub fn link_search(dir: &Path) {
        println!("cargo:rustc-link-search={}", dir.display());
    }
    /// Cause the build script to be re-run when any of the given env vars change.
    pub fn rerun_if_env(vars: &[&str]) {
        for v in vars {
            println!("cargo:rerun-if-env-changed={v}");
        }
    }
    /// Cause the build script to be re-run when the given path changes.
    pub fn rerun_if_changed<P: AsRef<Path>>(p: P) {
        println!("cargo:rerun-if-changed={}", p.as_ref().display());
    }
    /// Emit a Cargo build warning visible in build logs.
    pub fn warn(msg: impl AsRef<str>) {
        println!("cargo:warning={}", msg.as_ref());
    }
    /// Set a compile-time environment variable for the firmware crate.
    pub fn rustc_env(key: &str, value: &str) {
        println!("cargo:rustc-env={key}={value}");
    }
}

/* ------------------------------- Linker ---------------------------------- */

/// Linker-related helpers (copy `memory.x` to `OUT_DIR`).
mod linker {
    use super::*;
    /// Copy the workspace `memory.x` into `OUT_DIR` and add `OUT_DIR` to link search.
    pub fn install_memory_x(cfg: &Config) -> Result<()> {
        cargo::link_search(&cfg.out_dir);

        // Path to memory.x in the *main crate* (not build-support)
        let src = cfg.manifest_dir.join("memory.x");
        println!("cargo:rerun-if-changed={}", src.display());

        let bytes = fs::read(&src).with_context(|| format!("read {}", src.display()))?;
        fs::write(cfg.out_dir.join("memory.x"), bytes).context("write memory.x")?;
        Ok(())
    }
}

mod frontend;

mod payloads;

mod msc_image;

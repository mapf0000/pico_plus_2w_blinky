//! Build-support helpers invoked from the workspace `build.rs`.
//!
//! Installs the linker script and builds keyboard presets and the CDC agent image.

use anyhow::{Context, Result, bail};
use std::{env, fs, path::PathBuf};

/// Entry point called from the workspace `build.rs`.
///
/// Steps:
/// 1) Emit `rerun-if-*` hints for relevant env and files.
/// 2) Copy `memory.x` into `OUT_DIR` and expose it to the linker.
/// 3) Generate typed keyboard presets and embed the internal CDC agent image.
pub fn run() -> Result<()> {
    // Re-run on env var changes (identity and internal image label)
    cargo::rerun_if_env(&[
        env_consts::MSC_LABEL,
        env_consts::FIRMWARE_BUILD,
        env_consts::USB_SERIAL,
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
    if let Ok(serial) = env::var(env_consts::USB_SERIAL) {
        bootstrap::validate_serial(&serial)?;
        cargo::rustc_env(env_consts::USB_SERIAL, &serial);
    }
    cargo::rerun_if_changed(
        cfg.repo_root
            .join("crates/build-support/src/cdc_installer.sh"),
    );

    // 1) Linker script
    linker::install_memory_x(&cfg)?;

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
    /// Active profile (e.g., `debug` or `release`).
    profile: String,
    /// Active compilation target triple.
    target: String,
}

/// Environment variable names used by the build.
mod env_consts {
    pub const MSC_LABEL: &str = "PICO_MSC_LABEL";
    pub const FIRMWARE_BUILD: &str = "PICO_FIRMWARE_BUILD";
    pub const USB_SERIAL: &str = "PICO_USB_SERIAL";
}

impl Config {
    fn from_env() -> Result<Self> {
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").context("OUT_DIR missing")?);
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
        let repo_root = manifest_dir
            .parent()
            .context("workspace root not found (expected firmware crate at firmware/)")?
            .to_path_buf();
        let profile = env::var("PROFILE").unwrap_or_default();
        let target = env::var("TARGET").unwrap_or_default();

        Ok(Self {
            out_dir,
            manifest_dir,
            repo_root,
            profile,
            target,
        })
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

mod msc_image;

mod bootstrap;
mod presets;

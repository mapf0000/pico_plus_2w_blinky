//! Build-support helpers invoked from the workspace `build.rs`.
//!
//! Responsibilities (kept small and focused):
//! - Install the linker script (`memory.x`) into `OUT_DIR` and add it to the
//!   link search path for the firmware target.
//! - Ensure the Web UI (Yew) is built with Trunk to `frontend/dist/` when
//!   sources change, then copy a few stable-named assets into `OUT_DIR` and
//!   generate `frontend_static.rs` with `include_*` statements.
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
    // Re-run on env var changes (only size guards now)
    cargo::rerun_if_env(&[env_consts::WARN_BYTES, env_consts::MAX_BYTES]);

    // Re-run when these files change (build-support itself lives in its own crate)
    cargo::rerun_if_changed("build.rs");

    let cfg = Config::from_env()?;

    // 1) Linker script
    linker::install_memory_x(&cfg)?;

    // 2) Frontend pipeline (always required; auto-staleness detection)
    frontend::register_reruns(&cfg);
    frontend::prepare(&cfg)?;

    Ok(())
}

/* ----------------------------- Config & Env ------------------------------ */

#[derive(Debug)]
/// Build-time configuration captured from Cargo environment.
struct Config {
    /// Cargo's per-build output directory.
    out_dir: PathBuf,
    /// The manifest dir of the workspace root (contains `memory.x`, `frontend/`).
    manifest_dir: PathBuf,
    /// Path to the frontend crate.
    frontend_dir: PathBuf,
    /// Path to the DSL workspace (dsl-core, dsl-wasm, firmware-exec).
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
}

impl Config {
    fn from_env() -> Result<Self> {
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").context("OUT_DIR missing")?);
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
        let frontend_dir = manifest_dir.join("frontend");
        let dsl_dir = manifest_dir.join("dsl");
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

/* ------------------------------ Frontend --------------------------------- */

/// Frontend (Web UI) build and embedding pipeline.
mod frontend {
    use super::*;
    const DIST_DIR: &str = "dist";
    const GEN_RS: &str = "frontend_static.rs";
    const FP_FILE: &str = "frontend.fingerprint";

    /// Register broad change detection for the frontend sources.
    pub fn register_reruns(cfg: &Config) {
        // One broad watch is enough; Cargo will re-run build.rs when anything changes.
        cargo::rerun_if_changed("frontend");

        if cfg.dsl_dir.exists() {
            const DSL_PATHS: &[&str] = &[
                "dsl/Cargo.toml",
                "dsl/dsl-core/Cargo.toml",
                "dsl/dsl-core/src",
                "dsl/dsl-wasm/Cargo.toml",
                "dsl/dsl-wasm/src",
                "dsl/firmware-exec/Cargo.toml",
                "dsl/firmware-exec/src",
                "dsl/examples",
            ];
            for path in DSL_PATHS {
                cargo::rerun_if_changed(path);
            }
        }
    }

    /// Ensure the UI is up-to-date, embed assets, and generate `frontend_static.rs`.
    pub fn prepare(cfg: &Config) -> Result<()> {
        if !cfg.frontend_dir.exists() {
            bail!(
                "frontend directory not found at {} (expected a workspace member or sibling project)",
                cfg.frontend_dir.display()
            );
        }

        // Compute fingerprint of *sources* (excluding dist/ & friends)
        let frontend_fp = fingerprint_tree(
            &cfg.frontend_dir,
            &["dist", "target", ".git", "node_modules"],
        )?;
        let dsl_fp = fingerprint_optional(&cfg.dsl_dir, &["target", ".git", "pkg"])?;
        let cur_fp = format!("front={frontend_fp};dsl={dsl_fp}");
        let fp_path = cfg.out_dir.join(FP_FILE);
        let prev_fp = fs::read_to_string(&fp_path).ok();

        let dist = cfg.frontend_dir.join(DIST_DIR);
        let need_trunk = !dist.exists() || prev_fp.as_deref() != Some(&cur_fp);

        if need_trunk {
            if !try_trunk_build(cfg)? {
                bail!("frontend: dist is missing/stale and Trunk is not available or failed");
            }
        }

        // Always embed; will fail if dist missing
        embed_dist(cfg)?;

        // Record the fingerprint *after* a successful embed so future builds are fast.
        if let Err(e) = fs::write(&fp_path, &cur_fp) {
            cargo::warn(format!("frontend: failed to write fingerprint: {e}"));
        }
        Ok(())
    }

    /// Attempt to build the frontend via `trunk build --release`.
    /// Returns `Ok(true)` when Trunk succeeded, `Ok(false)` when Trunk was not
    /// found or failed (the caller may decide to continue if a valid `dist/` exists).
    fn try_trunk_build(cfg: &Config) -> Result<bool> {
        if which("trunk").is_err() {
            cargo::warn("frontend: Trunk not found; will use existing dist if present");
            return Ok(false);
        }

        // Separate target dir => avoids locking the outer build's target/
        let trunk_target = cfg.out_dir.join("trunk-target");
        let _ = std::fs::create_dir_all(&trunk_target);

        // Spawn Trunk with a “clean” env to avoid leaking embedded flags into wasm.
        let mut cmd = std::process::Command::new("trunk");
        cmd.arg("build")
            .arg("--release")
            .current_dir(&cfg.frontend_dir)
            .env("CARGO_TARGET_DIR", &trunk_target)
            // Avoid leaking MCU/outer Cargo state into the frontend build:
            .env_remove("TARGET")
            .env_remove("CARGO_BUILD_TARGET")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTDOCFLAGS")
            .env_remove("CARGO_ENCODED_RUSTDOCFLAGS")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            // Force wasm32 target for the inner cargo.
            .env("CARGO_BUILD_TARGET", "wasm32-unknown-unknown");

        // Avoid noisy cargo:warning output for normal trunk runs.

        let status = cmd.status().context("run trunk build")?;
        if status.success() {
            Ok(true)
        } else {
            cargo::warn(format!(
                "frontend: trunk build failed with status: {status}"
            ));
            Ok(false)
        }
    }

    /// Copy `dist/` artifacts to `OUT_DIR` with stable names and generate
    /// a small Rust module with `include_*` statements for serving over HTTP.
    fn embed_dist(cfg: &Config) -> Result<()> {
        let dist = cfg.frontend_dir.join(DIST_DIR);
        if !dist.exists() {
            bail!("frontend/{DIST_DIR} missing at {}", dist.display());
        }

        let js = pick_one_with_ext(&dist, "js")?;
        let wasm = pick_one_with_ext(&dist, "wasm")?;

        // Rewrite index.html references to stable /ui paths
        let index_src = dist.join("index.html");
        let mut index = fs::read_to_string(&index_src)
            .with_context(|| format!("read {}", index_src.display()))?;
        rewrite_paths(&mut index, &js, &wasm);

        // Copy to OUT_DIR with stable names
        fs::write(cfg.out_dir.join("frontend_index.html"), index)?;
        fs::copy(&js, cfg.out_dir.join("frontend_app.js")).context("copy js")?;
        fs::copy(&wasm, cfg.out_dir.join("frontend_app.wasm")).context("copy wasm")?;

        // Optional CSS
        let css_dist = dist.join("ui/style.css");
        if css_dist.exists() {
            fs::copy(&css_dist, cfg.out_dir.join("frontend_style.css"))
                .context("copy style.css")?;
        }

        // Size checks
        let wasm_len = fs::metadata(&wasm)?.len();
        if wasm_len > cfg.warn_bytes {
            cargo::warn(format!(
                "frontend WASM size {} bytes exceeds {} (consider opt-level=\"z\", lto, codegen-units=1)",
                wasm_len, cfg.warn_bytes
            ));
        }
        if let Some(max) = cfg.max_bytes {
            if wasm_len > max {
                bail!(
                    "frontend WASM size {} exceeds PICO_WASM_MAX_BYTES={} bytes",
                    wasm_len,
                    max
                );
            }
        }

        // Generate include file
        let gen_rs = cfg.out_dir.join(GEN_RS);
        let mut f = std::fs::File::create(&gen_rs).context("create frontend_static.rs")?;
        use std::io::Write;
        writeln!(
            f,
            "pub static INDEX_HTML: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/frontend_index.html\"));"
        )?;
        writeln!(
            f,
            "pub static APP_JS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_app.js\"));"
        )?;
        writeln!(
            f,
            "pub static APP_WASM: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_app.wasm\"));"
        )?;
        if cfg.out_dir.join("frontend_style.css").exists() {
            writeln!(
                f,
                "pub static STYLE_CSS: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/frontend_style.css\"));"
            )?;
        } else {
            writeln!(f, "pub static STYLE_CSS: &str = \"\";")?;
        }
        Ok(())
    }

    /// Find exactly one file in `dir` with the given extension.
    fn pick_one_with_ext(dir: &Path, ext: &str) -> Result<PathBuf> {
        let mut matches = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
            .collect::<Vec<_>>();
        matches.sort();
        match matches.len() {
            0 => bail!("could not find built .{ext} in {}", dir.display()),
            1 => Ok(matches.remove(0)),
            _ => bail!(
                "multiple .{ext} files in {}; make selection deterministic",
                dir.display()
            ),
        }
    }

    /// Normalize hashed asset names in `index.html` to stable `/ui/*` paths.
    fn rewrite_paths(index: &mut String, js: &Path, wasm: &Path) {
        let js_name = js.file_name().unwrap().to_string_lossy();
        let wasm_name = wasm.file_name().unwrap().to_string_lossy();

        *index = index.replace(&*js_name, "ui/app.js");
        *index = index.replace(&*wasm_name, "ui/app.wasm");
        *index = index.replace(&format!("/{}", js_name), "/ui/app.js");
        *index = index.replace(&format!("/{}", wasm_name), "/ui/app.wasm");
        *index = index.replace("/style.css", "/ui/style.css");
        if index.contains("/ui/ui/") {
            *index = index.replace("/ui/ui/", "/ui/");
        }
    }

    /* ------------------------- Fingerprinting ------------------------- */

    /// Compute a deterministic hash of all frontend sources (excluding build outputs).
    fn fingerprint_tree(root: &Path, skip_dirs: &[&str]) -> Result<String> {
        use blake3::Hasher;

        let mut files = Vec::<PathBuf>::new();
        collect_files(root, &mut files, skip_dirs)?;
        files.sort(); // stable order

        let mut hasher = Hasher::new();
        for path in files {
            // Include relative path and file contents for deterministic hashing
            hasher.update(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes(),
            );
            hasher.update(&fs::read(&path)?);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn fingerprint_optional(root: &Path, skip_dirs: &[&str]) -> Result<String> {
        if !root.exists() {
            return Ok(String::from("missing"));
        }
        fingerprint_tree(root, skip_dirs)
    }

    /// Recursively collect files under `dir`, skipping any directory in `skip_dirs`.
    fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, skip_dirs: &[&str]) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if skip_dirs.contains(&name) {
                    continue;
                }
                collect_files(&p, out, skip_dirs)?;
            } else {
                out.push(p);
            }
        }
        Ok(())
    }
}

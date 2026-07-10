//! Build-support helpers invoked from the workspace `build.rs`.
//!
//! Responsibilities (kept small and focused):
//! - Install the linker script (`memory.x`) into `OUT_DIR` and add it to the
//!   link search path for the firmware target.
//! - Ensure the Web UI (Yew) is built with Trunk to `apps/frontend/dist/` when
//!   sources change, then copy a few stable-named assets into `OUT_DIR` and
//!   generate `frontend_static.rs` with `include_*` statements.
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
    ]);

    // Re-run when these files change (build-support itself lives in its own crate)
    cargo::rerun_if_changed("build.rs");

    let cfg = Config::from_env()?;

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
        cargo::rerun_if_changed(&cfg.frontend_dir);

        if cfg.dsl_dir.exists() {
            let dsl_paths = [
                cfg.dsl_dir.join("dsl-core/Cargo.toml"),
                cfg.dsl_dir.join("dsl-core/src"),
                cfg.dsl_dir.join("dsl-wasm/Cargo.toml"),
                cfg.dsl_dir.join("dsl-wasm/src"),
                cfg.dsl_dir.join("firmware-exec/Cargo.toml"),
                cfg.dsl_dir.join("firmware-exec/src"),
                cfg.dsl_dir.join("examples"),
            ];
            for path in dsl_paths {
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

        if need_trunk && !try_trunk_build(cfg)? {
            bail!("frontend: dist is missing/stale and Trunk is not available or failed");
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
        std::fs::create_dir_all(&trunk_target).context("create Trunk target directory")?;

        // wasm-bindgen does not remove snippets that are no longer referenced.
        // Trunk can then copy and preload those stale files into a new dist,
        // producing an index whose integrity metadata does not match the current
        // module graph. Keep compiled Rust dependencies, but rebuild the binding
        // output and final dist from clean directories.
        let bindgen_output = trunk_target.join("wasm-bindgen");
        if bindgen_output.exists() {
            fs::remove_dir_all(&bindgen_output).context("clean stale wasm-bindgen output")?;
        }
        let dist = cfg.frontend_dir.join(DIST_DIR);
        if dist.exists() {
            fs::remove_dir_all(&dist).context("clean stale frontend dist")?;
        }

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
            .env_remove("NO_COLOR")
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

        // Static UI assets copied by Trunk.
        let css_dist = dist.join("ui/style.css");
        fs::copy(&css_dist, cfg.out_dir.join("frontend_style.css"))
            .with_context(|| format!("copy required frontend asset {}", css_dist.display()))?;
        let idb_js_dist = dist.join("ui/idb.js");
        fs::copy(&idb_js_dist, cfg.out_dir.join("frontend_idb.js"))
            .with_context(|| format!("copy required frontend asset {}", idb_js_dist.display()))?;

        // Size checks
        let wasm_len = fs::metadata(&wasm)?.len();
        if wasm_len > cfg.warn_bytes {
            cargo::warn(format!(
                "frontend WASM size {} bytes exceeds {} (consider opt-level=\"z\", lto, codegen-units=1)",
                wasm_len, cfg.warn_bytes
            ));
        }
        if let Some(max) = cfg.max_bytes
            && wasm_len > max
        {
            bail!(
                "frontend WASM size {} exceeds PICO_WASM_MAX_BYTES={} bytes",
                wasm_len,
                max
            );
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
        writeln!(
            f,
            "pub static STYLE_CSS: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/frontend_style.css\"));"
        )?;
        writeln!(
            f,
            "pub static IDB_JS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_idb.js\"));"
        )?;
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

/* -------------------------------- Payloads -------------------------------- */

mod payloads {
    use super::*;
    use core::fmt::Write as _;

    pub fn register_reruns(cfg: &Config) {
        let base = cfg.repo_root.join("crates/builtin-scripts");
        cargo::rerun_if_changed(base.join("Cargo.toml"));
        cargo::rerun_if_changed(base.join("src"));
    }

    pub fn prepare(cfg: &Config) -> Result<()> {
        let mut buf = String::new();
        writeln!(
            buf,
            "// @generated by build-support. Do not edit by hand.\nstatic PAYLOADS: &[Payload] = &["
        )
        .ok();

        for script in builtin_scripts::all() {
            let bytecode = compile_script(script)?;
            if bytecode.len() > bytecode_constants::MAX_BYTECODE {
                bail!(
                    "payload {} bytecode is {} bytes (max {})",
                    script.id,
                    bytecode.len(),
                    bytecode_constants::MAX_BYTECODE
                );
            }
            writeln!(buf, "    Payload {{").ok();
            writeln!(buf, "        name: {:?},", script.name).ok();
            writeln!(buf, "        detail: {:?},", script.description).ok();
            writeln!(buf, "        dsl: {:?},", script.dsl).ok();
            buf.push_str("        program: &[\n");
            fmt_bytes(&mut buf, &bytecode);
            buf.push_str("        ],\n");
            buf.push_str("    },\n");
        }

        buf.push_str("];\n");
        fs::write(cfg.out_dir.join("payloads_gen.rs"), buf)
            .context("write payloads_gen.rs into OUT_DIR")?;
        Ok(())
    }

    fn compile_script(script: &builtin_scripts::BuiltinScript) -> Result<Vec<u8>> {
        struct Provider;
        impl dsl_core::ScriptProvider for Provider {
            fn get<'a>(&'a self, id: &'a str) -> Option<&'a str> {
                builtin_scripts::lookup(id).map(|s| s as &str)
            }
        }

        let program = dsl_core::compile_and_link_with_required_layout(script.dsl, &Provider)
            .map_err(|e| compile_err("compile", script.id, e))?;
        let flat =
            dsl_core::lower_to_flat(&program).map_err(|e| compile_err("lower", script.id, e))?;
        let bytes = dsl_core::bytecode::encode(&flat).map_err(|_| {
            anyhow::anyhow!(
                "payload {} encode error: program exceeds maximum length",
                script.id
            )
        })?;
        Ok(bytes)
    }

    fn compile_err(stage: &str, id: &str, err: dsl_core::CompileError) -> anyhow::Error {
        anyhow::anyhow!(
            "payload {id} {stage} error: {} (line {})",
            err.code,
            err.span.line
        )
    }

    fn fmt_bytes(out: &mut String, bytes: &[u8]) {
        const PER_LINE: usize = 12;
        for chunk in bytes.chunks(PER_LINE) {
            out.push_str("            ");
            for (idx, b) in chunk.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "0x{:02x}", b);
            }
            out.push_str(",\n");
        }
    }
}

/* --------------------------- MSC Image Build ---------------------------- */

mod msc_image {
    use super::*;
    use std::fmt::Write as _;

    const IMAGE_BYTES: usize = 8 * 1024 * 1024;
    const BYTES_PER_SECTOR: usize = 512;
    const SECTORS_PER_CLUSTER: usize = 1;
    const CLUSTER_SIZE: usize = BYTES_PER_SECTOR * SECTORS_PER_CLUSTER;
    const RESERVED_SECTORS: usize = 1;
    const NUM_FATS: usize = 2;
    const ROOT_ENTRIES: usize = 512;
    const MEDIA_DESCRIPTOR: u8 = 0xF8;
    const VOLUME_LABEL_DEFAULT: &str = "PICO_AGENT";

    const ROOT_DIR_SECTORS: usize = (ROOT_ENTRIES * 32).div_ceil(BYTES_PER_SECTOR);

    const HOST_AGENT_NAME: &str = "HOSTAGNT";
    const README_NAME: &str = "README";
    const MISSING_NAME: &str = "MISSING";
    const TXT_EXT: &str = "TXT";

    struct TargetSpec {
        label: &'static str,
        required: bool,
        artifact_dir: &'static str,
        artifact_file: &'static str,
        volume_dir: &'static str,
        volume_file_name: &'static str,
        volume_file_ext: &'static str,
    }

    const TARGETS: &[TargetSpec] = &[
        TargetSpec {
            label: "macOS (aarch64)",
            required: true,
            artifact_dir: "aarch64-apple-darwin",
            artifact_file: "host-agent",
            volume_dir: "MAC",
            volume_file_name: HOST_AGENT_NAME,
            volume_file_ext: "",
        },
        TargetSpec {
            label: "Windows (x86_64)",
            required: false,
            artifact_dir: "x86_64-pc-windows-msvc",
            artifact_file: "host-agent.exe",
            volume_dir: "WIN",
            volume_file_name: HOST_AGENT_NAME,
            volume_file_ext: "EXE",
        },
        TargetSpec {
            label: "Linux (x86_64)",
            required: false,
            artifact_dir: "x86_64-unknown-linux-gnu",
            artifact_file: "host-agent",
            volume_dir: "LINUX",
            volume_file_name: HOST_AGENT_NAME,
            volume_file_ext: "",
        },
    ];

    #[derive(Clone)]
    struct FileSpec {
        name: [u8; 11],
        data: Vec<u8>,
    }

    struct DirSpec {
        name: [u8; 11],
        files: Vec<FileSpec>,
    }

    struct TargetStatus {
        volume_dir: &'static str,
        file_name: &'static str,
        file_ext: &'static str,
        present: bool,
    }

    pub fn register_reruns(cfg: &Config) {
        let host_agent = cfg.repo_root.join("apps/host-agent");
        cargo::rerun_if_changed(host_agent.join("artifacts"));
        cargo::rerun_if_changed(host_agent.join("Cargo.toml"));
    }

    pub fn prepare(cfg: &Config) -> Result<()> {
        if !cfg.target.starts_with("thumb") {
            return Ok(());
        }

        let artifacts_dir = cfg.repo_root.join("apps/host-agent/artifacts");
        let label = std::env::var(env_consts::MSC_LABEL)
            .ok()
            .unwrap_or_else(|| VOLUME_LABEL_DEFAULT.to_string());
        let label_bytes = normalize_label(&label)?;

        let mut root_files = Vec::new();
        let mut dir_specs = Vec::new();
        let mut status = Vec::new();

        for spec in TARGETS {
            let dir_name = short_name(spec.volume_dir, "")?;
            let mut files = Vec::new();
            let artifact_path = artifacts_dir
                .join(spec.artifact_dir)
                .join(spec.artifact_file);
            match fs::read(&artifact_path) {
                Ok(data) => {
                    let file_name = short_name(spec.volume_file_name, spec.volume_file_ext)?;
                    files.push(FileSpec {
                        name: file_name,
                        data,
                    });
                    status.push(TargetStatus {
                        volume_dir: spec.volume_dir,
                        file_name: spec.volume_file_name,
                        file_ext: spec.volume_file_ext,
                        present: true,
                    });
                }
                Err(_) => {
                    if spec.required {
                        cargo::warn(format!(
                            "msc: missing artifact for {} at {}",
                            spec.label,
                            artifact_path.display()
                        ));
                    }
                    let missing_name = short_name(MISSING_NAME, TXT_EXT)?;
                    let mut msg = String::new();
                    let _ = writeln!(msg, "Binary not included for {}.", spec.label);
                    files.push(FileSpec {
                        name: missing_name,
                        data: msg.into_bytes(),
                    });
                    status.push(TargetStatus {
                        volume_dir: spec.volume_dir,
                        file_name: spec.volume_file_name,
                        file_ext: spec.volume_file_ext,
                        present: false,
                    });
                }
            }

            dir_specs.push(DirSpec {
                name: dir_name,
                files,
            });
        }

        let readme_name = short_name(README_NAME, TXT_EXT)?;
        let readme = build_readme(&status, &label);
        root_files.push(FileSpec {
            name: readme_name,
            data: readme.into_bytes(),
        });

        let image = build_image(&label_bytes, root_files, dir_specs)?;
        let out_path = cfg.out_dir.join("host-agent.img");
        fs::write(&out_path, image).context("write host-agent.img")?;
        Ok(())
    }

    fn build_readme(status: &[TargetStatus], label: &str) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "PICO HOST AGENT USB IMAGE");
        let _ = writeln!(out, "Volume label: {}", label);
        let _ = writeln!(out, "This volume is read-only.");
        let _ = writeln!(out);
        let _ = writeln!(out, "Contents:");
        for target in status {
            if target.file_ext.is_empty() {
                let _ = writeln!(
                    out,
                    "- /{}/{}{}",
                    target.volume_dir,
                    target.file_name,
                    if target.present { "" } else { " (missing)" }
                );
            } else {
                let _ = writeln!(
                    out,
                    "- /{}/{}.{}{}",
                    target.volume_dir,
                    target.file_name,
                    target.file_ext,
                    if target.present { "" } else { " (missing)" }
                );
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "macOS quick start:");
        let _ = writeln!(
            out,
            "1) Copy /MAC/HOSTAGNT to a writable directory as host-agent"
        );
        let _ = writeln!(out, "2) chmod +x host-agent");
        let _ = writeln!(out, "3) ./host-agent vid=<VID> pid=<PID>");
        let _ = writeln!(out);
        let _ = writeln!(out, "To add other platforms, drop binaries into:");
        let _ = writeln!(out, "apps/host-agent/artifacts/<target>/");
        out
    }

    fn normalize_label(input: &str) -> Result<[u8; 11]> {
        let mut out = [b' '; 11];
        for (idx, ch) in input.bytes().take(11).enumerate() {
            out[idx] = match ch {
                b'a'..=b'z' => ch.to_ascii_uppercase(),
                b'A'..=b'Z' | b'0'..=b'9' | b' ' | b'_' => ch,
                _ => b'_',
            };
        }
        Ok(out)
    }

    fn short_name(name: &str, ext: &str) -> Result<[u8; 11]> {
        if name.is_empty() || name.len() > 8 || ext.len() > 3 {
            bail!("invalid short name: {name}.{ext}");
        }
        let mut out = [b' '; 11];
        for (idx, ch) in name.bytes().enumerate() {
            out[idx] = normalize_short_char(ch);
        }
        for (idx, ch) in ext.bytes().enumerate() {
            out[8 + idx] = normalize_short_char(ch);
        }
        Ok(out)
    }

    fn normalize_short_char(ch: u8) -> u8 {
        match ch {
            b'a'..=b'z' => ch.to_ascii_uppercase(),
            b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' => ch,
            _ => b'_',
        }
    }

    struct Layout {
        total_sectors: usize,
        sectors_per_fat: usize,
        data_start_sector: usize,
        cluster_count: usize,
    }

    fn compute_layout() -> Result<Layout> {
        if !IMAGE_BYTES.is_multiple_of(BYTES_PER_SECTOR) {
            bail!("MSC image size must be sector-aligned");
        }
        let total_sectors = IMAGE_BYTES / BYTES_PER_SECTOR;
        let mut sectors_per_fat = 1usize;
        let cluster_count;
        loop {
            let data_sectors = total_sectors
                .saturating_sub(RESERVED_SECTORS + ROOT_DIR_SECTORS + NUM_FATS * sectors_per_fat);
            let clusters = data_sectors / SECTORS_PER_CLUSTER;
            let fat_bytes = (clusters + 2) * 2;
            let new_spf = fat_bytes.div_ceil(BYTES_PER_SECTOR);
            if new_spf == sectors_per_fat {
                cluster_count = clusters;
                break;
            }
            sectors_per_fat = new_spf;
        }

        if !(4085..=65524).contains(&cluster_count) {
            bail!("MSC image cluster count out of FAT16 range");
        }

        let data_start_sector = RESERVED_SECTORS + NUM_FATS * sectors_per_fat + ROOT_DIR_SECTORS;

        Ok(Layout {
            total_sectors,
            sectors_per_fat,
            data_start_sector,
            cluster_count,
        })
    }

    fn build_image(
        label: &[u8; 11],
        root_files: Vec<FileSpec>,
        dirs: Vec<DirSpec>,
    ) -> Result<Vec<u8>> {
        let layout = compute_layout()?;
        let mut image = vec![0u8; IMAGE_BYTES];

        write_boot_sector(&mut image[..BYTES_PER_SECTOR], label, &layout);

        let mut allocator = Allocator::new(layout.cluster_count);

        let mut root_allocs = Vec::new();
        for file in root_files {
            let (cluster, size) = allocator.alloc_file(&file.data)?;
            root_allocs.push(FileAlloc {
                name: file.name,
                attr: 0x01,
                cluster,
                size,
                data: file.data,
            });
        }

        let mut dir_allocs = Vec::new();
        for dir in dirs {
            let dir_cluster = allocator.alloc_dir()?;
            let mut files = Vec::new();
            for file in dir.files {
                let (cluster, size) = allocator.alloc_file(&file.data)?;
                files.push(FileAlloc {
                    name: file.name,
                    attr: 0x01,
                    cluster,
                    size,
                    data: file.data,
                });
            }
            dir_allocs.push(DirAlloc {
                name: dir.name,
                cluster: dir_cluster,
                files,
            });
        }

        let fat0_start = RESERVED_SECTORS * BYTES_PER_SECTOR;
        let fat_size_bytes = layout.sectors_per_fat * BYTES_PER_SECTOR;
        write_fat(
            &mut image[fat0_start..fat0_start + fat_size_bytes],
            &allocator.fat,
        );
        let fat1_start = fat0_start + fat_size_bytes;
        write_fat(
            &mut image[fat1_start..fat1_start + fat_size_bytes],
            &allocator.fat,
        );

        let root_dir_start =
            (RESERVED_SECTORS + NUM_FATS * layout.sectors_per_fat) * BYTES_PER_SECTOR;
        let root_dir_bytes = ROOT_DIR_SECTORS * BYTES_PER_SECTOR;
        let mut root_entries = Vec::new();
        root_entries.push(DirEntry::volume_label(*label));
        for dir in &dir_allocs {
            root_entries.push(DirEntry::directory(dir.name, dir.cluster));
        }
        for file in &root_allocs {
            root_entries.push(DirEntry::file(
                file.name,
                file.attr,
                file.cluster,
                file.size,
            ));
        }
        write_dir_entries(
            &mut image[root_dir_start..root_dir_start + root_dir_bytes],
            &root_entries,
        )?;

        let data_start = layout.data_start_sector * BYTES_PER_SECTOR;
        for file in &root_allocs {
            write_file_data(&mut image, data_start, file.cluster, &file.data)?;
        }
        for dir in &dir_allocs {
            let cluster_offset = cluster_offset(data_start, dir.cluster)?;
            let mut entries = Vec::new();
            entries.push(DirEntry::dot(dir.cluster));
            entries.push(DirEntry::dotdot(0));
            for file in &dir.files {
                entries.push(DirEntry::file(
                    file.name,
                    file.attr,
                    file.cluster,
                    file.size,
                ));
                write_file_data(&mut image, data_start, file.cluster, &file.data)?;
            }
            write_dir_entries(
                &mut image[cluster_offset..cluster_offset + CLUSTER_SIZE],
                &entries,
            )?;
        }

        Ok(image)
    }

    struct Allocator {
        next_cluster: u16,
        cluster_count: usize,
        fat: Vec<u16>,
    }

    impl Allocator {
        fn new(cluster_count: usize) -> Self {
            let mut fat = vec![0u16; cluster_count + 2];
            fat[0] = 0xFFF8;
            fat[1] = 0xFFFF;
            Self {
                next_cluster: 2,
                cluster_count,
                fat,
            }
        }

        fn alloc_dir(&mut self) -> Result<u16> {
            self.alloc_clusters(1)
        }

        fn alloc_file(&mut self, data: &[u8]) -> Result<(u16, u32)> {
            let size = data.len() as u32;
            if size == 0 {
                return Ok((0, 0));
            }
            let clusters = data.len().div_ceil(CLUSTER_SIZE);
            let cluster = self.alloc_clusters(clusters)?;
            Ok((cluster, size))
        }

        fn alloc_clusters(&mut self, clusters: usize) -> Result<u16> {
            let start = self.next_cluster as usize;
            let end = start + clusters - 1;
            let max_cluster = self.cluster_count + 1;
            if end > max_cluster {
                bail!("MSC image too small for files");
            }

            let start_cluster = self.next_cluster;
            for offset in 0..clusters {
                let cur = start_cluster + offset as u16;
                let next = if offset + 1 == clusters {
                    0xFFFF
                } else {
                    cur + 1
                };
                self.fat[cur as usize] = next;
            }
            self.next_cluster = start_cluster + clusters as u16;
            Ok(start_cluster)
        }
    }

    struct FileAlloc {
        name: [u8; 11],
        attr: u8,
        cluster: u16,
        size: u32,
        data: Vec<u8>,
    }

    struct DirAlloc {
        name: [u8; 11],
        cluster: u16,
        files: Vec<FileAlloc>,
    }

    #[derive(Clone, Copy)]
    struct DirEntry {
        name: [u8; 11],
        attr: u8,
        cluster: u16,
        size: u32,
    }

    impl DirEntry {
        fn volume_label(label: [u8; 11]) -> Self {
            Self {
                name: label,
                attr: 0x08,
                cluster: 0,
                size: 0,
            }
        }

        fn directory(name: [u8; 11], cluster: u16) -> Self {
            Self {
                name,
                attr: 0x10,
                cluster,
                size: 0,
            }
        }

        fn file(name: [u8; 11], attr: u8, cluster: u16, size: u32) -> Self {
            Self {
                name,
                attr,
                cluster,
                size,
            }
        }

        fn dot(cluster: u16) -> Self {
            let mut name = [b' '; 11];
            name[0] = b'.';
            Self::directory(name, cluster)
        }

        fn dotdot(cluster: u16) -> Self {
            let mut name = [b' '; 11];
            name[0] = b'.';
            name[1] = b'.';
            Self::directory(name, cluster)
        }
    }

    fn write_boot_sector(buf: &mut [u8], label: &[u8; 11], layout: &Layout) {
        buf.fill(0);
        buf[0] = 0xEB;
        buf[1] = 0x3C;
        buf[2] = 0x90;
        buf[3..11].copy_from_slice(b"MSDOS5.0");
        buf[11..13].copy_from_slice(&(BYTES_PER_SECTOR as u16).to_le_bytes());
        buf[13] = SECTORS_PER_CLUSTER as u8;
        buf[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
        buf[16] = NUM_FATS as u8;
        buf[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
        buf[19..21].copy_from_slice(&(layout.total_sectors as u16).to_le_bytes());
        buf[21] = MEDIA_DESCRIPTOR;
        buf[22..24].copy_from_slice(&(layout.sectors_per_fat as u16).to_le_bytes());
        buf[24..26].copy_from_slice(&63u16.to_le_bytes());
        buf[26..28].copy_from_slice(&255u16.to_le_bytes());
        buf[28..32].copy_from_slice(&0u32.to_le_bytes());
        buf[32..36].copy_from_slice(&0u32.to_le_bytes());
        buf[36] = 0x80;
        buf[37] = 0;
        buf[38] = 0x29;
        buf[39..43].copy_from_slice(&0x12345678u32.to_le_bytes());
        buf[43..54].copy_from_slice(label);
        buf[54..62].copy_from_slice(b"FAT16   ");
        buf[510] = 0x55;
        buf[511] = 0xAA;
    }

    fn write_fat(buf: &mut [u8], fat: &[u16]) {
        buf.fill(0);
        for (idx, entry) in fat.iter().enumerate() {
            let offset = idx * 2;
            if offset + 1 >= buf.len() {
                break;
            }
            buf[offset] = (*entry & 0xFF) as u8;
            buf[offset + 1] = (entry >> 8) as u8;
        }
    }

    fn write_dir_entries(buf: &mut [u8], entries: &[DirEntry]) -> Result<()> {
        let mut offset = 0usize;
        for entry in entries {
            if offset + 32 > buf.len() {
                bail!("directory entry overflow");
            }
            write_dir_entry(&mut buf[offset..offset + 32], entry);
            offset += 32;
        }
        Ok(())
    }

    fn write_dir_entry(buf: &mut [u8], entry: &DirEntry) {
        buf.fill(0);
        buf[..11].copy_from_slice(&entry.name);
        buf[11] = entry.attr;
        buf[26..28].copy_from_slice(&entry.cluster.to_le_bytes());
        buf[28..32].copy_from_slice(&entry.size.to_le_bytes());
    }

    fn write_file_data(
        image: &mut [u8],
        data_start: usize,
        cluster: u16,
        data: &[u8],
    ) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut offset = cluster_offset(data_start, cluster)?;
        for chunk in data.chunks(CLUSTER_SIZE) {
            let end = offset + chunk.len();
            if end > image.len() {
                bail!("MSC image overflow");
            }
            image[offset..end].copy_from_slice(chunk);
            offset += CLUSTER_SIZE;
        }
        Ok(())
    }

    fn cluster_offset(data_start: usize, cluster: u16) -> Result<usize> {
        if cluster < 2 {
            bail!("invalid cluster {cluster}");
        }
        let idx = (cluster as usize - 2) * CLUSTER_SIZE;
        Ok(data_start + idx)
    }
}

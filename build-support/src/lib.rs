use anyhow::{bail, Context, Result};
use std::{
    env, fs,
    path::{Path, PathBuf},
};
use which::which;

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
struct Config {
    out_dir: PathBuf,
    manifest_dir: PathBuf,
    frontend_dir: PathBuf,
    profile: String,
    target: String,
    warn_bytes: u64,
    max_bytes: Option<u64>,
}

mod env_consts {
    pub const WARN_BYTES: &str = "PICO_WASM_WARN_BYTES";
    pub const MAX_BYTES: &str = "PICO_WASM_MAX_BYTES";
}

impl Config {
    fn from_env() -> Result<Self> {
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").context("OUT_DIR missing")?);
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
        let frontend_dir = manifest_dir.join("frontend");
        let profile = env::var("PROFILE").unwrap_or_default();
        let target = env::var("TARGET").unwrap_or_default();

        let warn_bytes = env::var(env_consts::WARN_BYTES)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_200_000);
        let max_bytes = env::var(env_consts::MAX_BYTES).ok().and_then(|s| s.parse().ok());

        Ok(Self {
            out_dir,
            manifest_dir,
            frontend_dir,
            profile,
            target,
            warn_bytes,
            max_bytes,
        })
    }

    #[allow(dead_code)]
    fn is_embedded_release(&self) -> bool {
        self.profile == "release" && self.target.starts_with("thumb")
    }
}

/* -------------------------------- Cargo ---------------------------------- */

mod cargo {
    use std::path::Path;

    pub fn link_search(dir: &Path) {
        println!("cargo:rustc-link-search={}", dir.display());
    }
    pub fn rerun_if_env(vars: &[&str]) {
        for v in vars {
            println!("cargo:rerun-if-env-changed={v}");
        }
    }
    pub fn rerun_if_changed<P: AsRef<Path>>(p: P) {
        println!("cargo:rerun-if-changed={}", p.as_ref().display());
    }
    pub fn warn(msg: impl AsRef<str>) {
        println!("cargo:warning={}", msg.as_ref());
    }
}

/* ------------------------------- Linker ---------------------------------- */

mod linker {
    use super::*;
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

mod frontend {
    use super::*;
    const DIST_DIR: &str = "dist";
    const GEN_RS: &str = "frontend_static.rs";
    const FP_FILE: &str = "frontend.fingerprint";

    pub fn register_reruns(_cfg: &Config) {
        // One broad watch is enough; Cargo will re-run build.rs when anything changes.
        cargo::rerun_if_changed("frontend");
        // If you prefer granular:
        // cargo::rerun_if_changed("frontend/Cargo.toml");
        // cargo::rerun_if_changed("frontend/Cargo.lock");
        // cargo::rerun_if_changed("frontend/Trunk.toml");
        // cargo::rerun_if_changed("frontend/index.html");
        // cargo::rerun_if_changed("frontend/src");
        // cargo::rerun_if_changed("frontend/ui");
    }

    pub fn prepare(cfg: &Config) -> Result<()> {
        if !cfg.frontend_dir.exists() {
            bail!(
                "frontend directory not found at {} (expected a workspace member or sibling project)",
                cfg.frontend_dir.display()
            );
        }

        // Compute fingerprint of *sources* (excluding dist/ & friends)
        let cur_fp = fingerprint_frontend(&cfg.frontend_dir)?;
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

    fn try_trunk_build(cfg: &Config) -> Result<bool> {
        if which("trunk").is_err() {
            cargo::warn("frontend: Trunk not found; will use existing dist if present");
            return Ok(false);
        }

        // Separate target dir => avoids locking the outer build's target/
        let trunk_target = cfg.out_dir.join("trunk-target");
        let _ = std::fs::create_dir_all(&trunk_target);

        // Spawn trunk with a “clean” env
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

        println!(
            "cargo:warning=frontend: running trunk with CARGO_TARGET_DIR={}",
            trunk_target.display()
        );

        let status = cmd.status().context("run trunk build")?;
        if status.success() {
            Ok(true)
        } else {
            cargo::warn(format!("frontend: trunk build failed with status: {status}"));
            Ok(false)
        }
    }

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

    fn fingerprint_frontend(root: &Path) -> Result<String> {
        use blake3::Hasher;

        let mut files = Vec::<PathBuf>::new();
        collect_files(root, &mut files, &["dist", "target", ".git", "node_modules"])?;
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

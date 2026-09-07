//! Build, fingerprint, and embed the Yew/Trunk frontend.

use super::*;
use flate2::{Compression, GzBuilder};
use std::io::Write as _;
const RELEASE_DIST_DIR: &str = "frontend-dist";
const GEN_RS: &str = "frontend_static.rs";
const FP_FILE: &str = "frontend.fingerprint";

/// Register broad change detection for the frontend sources.
pub fn register_reruns(cfg: &Config) {
    // One broad watch is enough; Cargo will re-run build.rs when anything changes.
    cargo::rerun_if_changed(&cfg.frontend_dir);
    cargo::rerun_if_changed(&cfg.python_worker_dir);
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
    let python_fp = fingerprint_tree(&cfg.python_worker_dir, &["target", ".git", "pkg"])?;
    let cur_fp = format!("front={frontend_fp};python={python_fp}");
    let fp_path = cfg.out_dir.join(FP_FILE);
    let prev_fp = fs::read_to_string(&fp_path).ok();

    // Keep the firmware release bundle isolated from apps/frontend/dist.
    // `trunk serve` also writes to that shared development directory and can
    // replace an optimized bundle without changing any source fingerprints.
    let dist = release_dist(&cfg.out_dir);
    let worker_ready = cfg.out_dir.join("frontend_python_runtime.wasm.gz").exists()
        && cfg.out_dir.join("frontend_python_runtime.js").exists()
        && cfg.out_dir.join("frontend_python_worker.js").exists();
    let need_trunk = !dist.exists() || prev_fp.as_deref() != Some(&cur_fp);

    if need_trunk && !try_trunk_build(cfg)? {
        bail!(
            "frontend: isolated release bundle is missing/stale and Trunk is not available or failed"
        );
    }
    if (!worker_ready || prev_fp.as_deref() != Some(&cur_fp)) && !try_python_worker_build(cfg)? {
        bail!("frontend: RustPython Worker bundle is missing/stale and could not be built");
    }

    // Always embed; will fail if dist missing
    embed_dist(cfg)?;

    // Record the fingerprint *after* a successful embed so future builds are fast.
    if let Err(e) = fs::write(&fp_path, &cur_fp) {
        cargo::warn(format!("frontend: failed to write fingerprint: {e}"));
    }
    Ok(())
}

fn try_python_worker_build(cfg: &Config) -> Result<bool> {
    if which("cargo").is_err() || which("wasm-bindgen").is_err() {
        cargo::warn("frontend: cargo or wasm-bindgen is unavailable for Python Worker build");
        return Ok(false);
    }
    let target_dir = cfg.out_dir.join("python-worker-target");
    let bindgen_dir = cfg.out_dir.join("python-worker-bindgen");
    fs::create_dir_all(&target_dir).context("create Python Worker target directory")?;
    if bindgen_dir.exists() {
        fs::remove_dir_all(&bindgen_dir).context("clean Python Worker bindgen directory")?;
    }

    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(cfg.python_worker_dir.join("Cargo.toml"))
        .arg("--release")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("TARGET")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTDOCFLAGS")
        .env_remove("CARGO_ENCODED_RUSTDOCFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .status()
        .context("build RustPython Worker")?;
    if !status.success() {
        cargo::warn(format!(
            "frontend: Python Worker cargo build failed: {status}"
        ));
        return Ok(false);
    }

    let wasm = target_dir.join("wasm32-unknown-unknown/release/python_worker.wasm");
    let status = std::process::Command::new("wasm-bindgen")
        .arg("--target")
        .arg("web")
        .arg("--out-name")
        .arg("python_runtime")
        .arg("--out-dir")
        .arg(&bindgen_dir)
        .arg(&wasm)
        .status()
        .context("run wasm-bindgen for RustPython Worker")?;
    if !status.success() {
        cargo::warn(format!(
            "frontend: Python Worker wasm-bindgen failed: {status}"
        ));
        return Ok(false);
    }

    fs::copy(
        bindgen_dir.join("python_runtime.js"),
        cfg.out_dir.join("frontend_python_runtime.js"),
    )
    .context("copy Python runtime JavaScript")?;
    fs::copy(
        cfg.python_worker_dir.join("worker.js"),
        cfg.out_dir.join("frontend_python_worker.js"),
    )
    .context("copy Python Worker driver")?;

    let raw_wasm = fs::read(bindgen_dir.join("python_runtime_bg.wasm"))
        .context("read bound Python runtime WASM")?;
    let output = std::fs::File::create(cfg.out_dir.join("frontend_python_runtime.wasm.gz"))
        .context("create compressed Python runtime WASM")?;
    let mut encoder = GzBuilder::new().mtime(0).write(output, Compression::best());
    encoder.write_all(&raw_wasm)?;
    encoder.finish()?;
    let compressed_len = fs::metadata(cfg.out_dir.join("frontend_python_runtime.wasm.gz"))?.len();
    if compressed_len > cfg.python_warn_bytes {
        cargo::warn(format!(
            "compressed Python Worker WASM size {compressed_len} bytes exceeds {}",
            cfg.python_warn_bytes
        ));
    }
    if compressed_len > cfg.python_max_bytes {
        bail!(
            "compressed Python Worker WASM size {compressed_len} exceeds PICO_PYTHON_WASM_MAX_BYTES={} bytes",
            cfg.python_max_bytes
        );
    }
    Ok(true)
}

/// Attempt to build the frontend via `trunk build --release`.
/// Returns `Ok(true)` when Trunk succeeded and `Ok(false)` when Trunk was not
/// found or failed.
fn try_trunk_build(cfg: &Config) -> Result<bool> {
    if which("trunk").is_err() {
        cargo::warn("frontend: Trunk not found");
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
    let dist = release_dist(&cfg.out_dir);
    if dist.exists() {
        fs::remove_dir_all(&dist).context("clean stale firmware frontend dist")?;
    }

    // Spawn Trunk with a “clean” env to avoid leaking embedded flags into wasm.
    let mut cmd = std::process::Command::new("trunk");
    cmd.arg("build")
        .arg("--release")
        .arg("--dist")
        .arg(&dist)
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

/// Copy the isolated release artifacts to stable names in `OUT_DIR` and
/// generate a small Rust module with `include_*` statements for serving over
/// HTTP.
fn embed_dist(cfg: &Config) -> Result<()> {
    let dist = release_dist(&cfg.out_dir);
    if !dist.exists() {
        bail!(
            "firmware frontend release bundle missing at {}",
            dist.display()
        );
    }

    let js = pick_one_with_ext(&dist, "js")?;
    let wasm = pick_one_with_ext(&dist, "wasm")?;
    let css = pick_one_with_ext(&dist, "css")?;

    // Rewrite index.html references to stable /ui paths
    let index_src = dist.join("index.html");
    let mut index =
        fs::read_to_string(&index_src).with_context(|| format!("read {}", index_src.display()))?;
    rewrite_paths(&mut index, &js, &wasm, &css);

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
    writeln!(
        f,
        "pub static PYTHON_WORKER_JS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_python_worker.js\"));"
    )?;
    writeln!(
        f,
        "pub static PYTHON_RUNTIME_JS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_python_runtime.js\"));"
    )?;
    writeln!(
        f,
        "pub static PYTHON_RUNTIME_WASM_GZIP: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_python_runtime.wasm.gz\"));"
    )?;
    Ok(())
}

fn release_dist(out_dir: &Path) -> PathBuf {
    out_dir.join(RELEASE_DIST_DIR)
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
fn rewrite_paths(index: &mut String, js: &Path, wasm: &Path, css: &Path) {
    let js_name = js.file_name().unwrap().to_string_lossy();
    let wasm_name = wasm.file_name().unwrap().to_string_lossy();
    let css_name = css.file_name().unwrap().to_string_lossy();

    *index = index.replace(&*js_name, "ui/app.js");
    *index = index.replace(&*wasm_name, "ui/app.wasm");
    *index = index.replace(&format!("/{}", js_name), "/ui/app.js");
    *index = index.replace(&format!("/{}", wasm_name), "/ui/app.wasm");
    *index = index.replace(&*css_name, "ui/style.css");
    *index = index.replace(&format!("/{}", css_name), "/ui/style.css");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_release_dist_is_scoped_to_cargo_out_dir() {
        let out_dir = Path::new("cargo-out");

        assert_eq!(release_dist(out_dir), out_dir.join("frontend-dist"));
        assert_ne!(release_dist(out_dir), Path::new("apps/frontend/dist"));
    }
}

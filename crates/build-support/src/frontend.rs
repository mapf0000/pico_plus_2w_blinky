//! Build, fingerprint, and embed the Yew/Trunk frontend.

use super::*;
use flate2::{Compression, GzBuilder};
use std::io::Write as _;
const RELEASE_DIST_DIR: &str = "frontend-dist";
const GEN_RS: &str = "frontend_static.rs";
const FP_FILE: &str = "frontend.fingerprint";
const SHARED_KEYBOARD_INPUTS: &[&str] = &["crates/keyboard-core", "crates/bytecode-constants"];

/// Register broad change detection for the frontend sources.
pub fn register_reruns(cfg: &Config) {
    // One broad watch is enough; Cargo will re-run build.rs when anything changes.
    cargo::rerun_if_changed(&cfg.frontend_dir);
    cargo::rerun_if_changed(&cfg.python_worker_dir);
    for input in SHARED_KEYBOARD_INPUTS {
        cargo::rerun_if_changed(cfg.repo_root.join(input));
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
    let python_fp = fingerprint_tree(&cfg.python_worker_dir, &["target", ".git", "pkg"])?;
    let keyboard_fp = shared_keyboard_fingerprint(&cfg.repo_root)?;
    let cur_fp = format!("front={frontend_fp};python={python_fp};keyboard={keyboard_fp}");
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
    write_gzip(
        &cfg.out_dir.join("frontend_python_runtime.wasm.gz"),
        &raw_wasm,
    )?;
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

    // Store only gzip representations in firmware; HTTP routes preserve the
    // original media types and let the browser decompress them.
    write_gzip(
        &cfg.out_dir.join("frontend_index.html.gz"),
        index.as_bytes(),
    )?;
    copy_gzip(&js, &cfg.out_dir.join("frontend_app.js.gz"))?;
    copy_gzip(&wasm, &cfg.out_dir.join("frontend_app.wasm.gz"))?;

    // Static UI assets copied by Trunk.
    let css_dist = dist.join("ui/style.css");
    copy_gzip(&css_dist, &cfg.out_dir.join("frontend_style.css.gz"))?;
    let idb_js_dist = dist.join("ui/idb.js");
    copy_gzip(&idb_js_dist, &cfg.out_dir.join("frontend_idb.js.gz"))?;
    for name in ["frontend_python_runtime.js", "frontend_python_worker.js"] {
        copy_gzip(
            &cfg.out_dir.join(name),
            &cfg.out_dir.join(format!("{name}.gz")),
        )?;
    }

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
    for (symbol, file) in [
        ("INDEX_HTML_GZIP", "frontend_index.html.gz"),
        ("APP_JS_GZIP", "frontend_app.js.gz"),
        ("APP_WASM_GZIP", "frontend_app.wasm.gz"),
        ("STYLE_CSS_GZIP", "frontend_style.css.gz"),
        ("IDB_JS_GZIP", "frontend_idb.js.gz"),
        ("PYTHON_WORKER_JS_GZIP", "frontend_python_worker.js.gz"),
        ("PYTHON_RUNTIME_JS_GZIP", "frontend_python_runtime.js.gz"),
        (
            "PYTHON_RUNTIME_WASM_GZIP",
            "frontend_python_runtime.wasm.gz",
        ),
    ] {
        writeln!(
            f,
            "pub static {symbol}: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}\"));"
        )?;
    }
    Ok(())
}

fn copy_gzip(source: &Path, destination: &Path) -> Result<()> {
    let bytes = fs::read(source).with_context(|| format!("read asset {}", source.display()))?;
    write_gzip(destination, &bytes)
}

fn write_gzip(destination: &Path, bytes: &[u8]) -> Result<()> {
    let output = fs::File::create(destination)
        .with_context(|| format!("create compressed asset {}", destination.display()))?;
    // No filename or timestamp: identical source bytes produce identical assets.
    let mut encoder = GzBuilder::new().mtime(0).write(output, Compression::best());
    encoder.write_all(bytes)?;
    encoder.finish()?;
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

fn shared_keyboard_fingerprint(repo_root: &Path) -> Result<String> {
    let mut result = String::new();
    for input in SHARED_KEYBOARD_INPUTS {
        result.push_str(input);
        result.push('=');
        result.push_str(&fingerprint_tree(
            &repo_root.join(input),
            &["target", ".git"],
        )?);
        result.push(';');
    }
    Ok(result)
}

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
    use std::io::Read as _;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!("pico-assets-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn keyboard_dependency_edits_invalidate_embedded_worker_assets() {
        let root = TestDir::new();
        for input in SHARED_KEYBOARD_INPUTS {
            fs::create_dir_all(root.0.join(input).join("src")).unwrap();
            fs::write(root.0.join(input).join("src/lib.rs"), b"original").unwrap();
        }
        let original = shared_keyboard_fingerprint(&root.0).unwrap();
        let keyboard = root.0.join("crates/keyboard-core");
        fs::write(keyboard.join("src/lib.rs"), b"corrected layout").unwrap();
        let corrected = shared_keyboard_fingerprint(&root.0).unwrap();
        assert_ne!(original, corrected);
        fs::create_dir_all(keyboard.join("target")).unwrap();
        fs::write(keyboard.join("target/output"), b"generated").unwrap();
        assert_eq!(corrected, shared_keyboard_fingerprint(&root.0).unwrap());
        fs::write(
            root.0.join("crates/bytecode-constants/src/lib.rs"),
            b"updated constant",
        )
        .unwrap();
        assert_ne!(corrected, shared_keyboard_fingerprint(&root.0).unwrap());
    }

    #[test]
    fn compressed_embedding_preserves_assets_paths_and_size_gates() {
        let root = TestDir::new();
        let mut cfg = Config {
            out_dir: root.0.clone(),
            manifest_dir: root.0.clone(),
            repo_root: root.0.clone(),
            frontend_dir: root.0.clone(),
            python_worker_dir: root.0.clone(),
            profile: "release".into(),
            target: "thumbv8m.main-none-eabihf".into(),
            warn_bytes: u64::MAX,
            max_bytes: None,
            python_warn_bytes: u64::MAX,
            python_max_bytes: u64::MAX,
        };
        let dist = release_dist(&cfg.out_dir);
        fs::create_dir_all(dist.join("ui")).unwrap();
        fs::write(
            dist.join("index.html"),
            b"<script src=\"app-123.js\"></script><link href=\"style-123.css\">app-123.wasm",
        )
        .unwrap();
        let assets: &[(&str, &str, &[u8])] = &[
            ("app-123.js", "frontend_app.js.gz", b"export const app = 1;"),
            ("app-123.wasm", "frontend_app.wasm.gz", b"\0asm\x01\0\0\0"),
            (
                "ui/style.css",
                "frontend_style.css.gz",
                b"body { color: red; }",
            ),
            ("ui/idb.js", "frontend_idb.js.gz", b"export const idb = 2;"),
        ];
        for (source, _, bytes) in assets {
            fs::write(dist.join(source), bytes).unwrap();
        }
        fs::write(dist.join("style-123.css"), b"body { color: red; }").unwrap();
        let worker_assets: &[(&str, &[u8])] = &[
            (
                "frontend_python_runtime.js",
                b"export default async function init() {}",
            ),
            (
                "frontend_python_worker.js",
                b"import init from '/ui/python-runtime.js';",
            ),
        ];
        for (name, bytes) in worker_assets {
            fs::write(cfg.out_dir.join(name), bytes).unwrap();
        }
        let runtime_wasm = b"\0asm\x01\0\0\0";
        write_gzip(
            &cfg.out_dir.join("frontend_python_runtime.wasm.gz"),
            runtime_wasm,
        )
        .unwrap();

        embed_dist(&cfg).unwrap();
        let generated = fs::read_to_string(cfg.out_dir.join(GEN_RS)).unwrap();
        assert_eq!(generated.lines().count(), 8);
        assert!(generated.lines().all(|line| line.contains(".gz\"))")));
        let expected_index =
            b"<script src=\"ui/app.js\"></script><link href=\"ui/style.css\">ui/app.wasm";
        let mut expected = vec![(
            "frontend_index.html.gz".to_owned(),
            expected_index.as_slice(),
        )];
        expected.extend(
            assets
                .iter()
                .map(|(_, destination, bytes)| ((*destination).into(), *bytes)),
        );
        expected.extend(
            worker_assets
                .iter()
                .map(|(name, bytes)| (format!("{name}.gz"), *bytes)),
        );
        expected.push((
            "frontend_python_runtime.wasm.gz".into(),
            runtime_wasm.as_slice(),
        ));
        let mut first_build = Vec::new();
        for (name, original) in &expected {
            let compressed = fs::read(cfg.out_dir.join(name)).unwrap();
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(compressed.as_slice())
                .read_to_end(&mut decoded)
                .unwrap();
            assert_eq!(&decoded, original, "{name}");
            first_build.push(compressed);
        }
        // Cached Worker bundles and frontend dist still get deterministic gzip
        // assets whenever build.rs embeds them again.
        embed_dist(&cfg).unwrap();
        for ((name, _), previous) in expected.iter().zip(first_build) {
            assert_eq!(fs::read(cfg.out_dir.join(name)).unwrap(), previous);
        }
        cfg.max_bytes = Some(7);
        assert!(
            embed_dist(&cfg)
                .unwrap_err()
                .to_string()
                .contains("PICO_WASM_MAX_BYTES=7")
        );
        fs::remove_file(dist.join("ui/idb.js")).unwrap();
        assert!(embed_dist(&cfg).unwrap_err().to_string().contains("idb.js"));
    }

    #[test]
    fn firmware_release_dist_is_scoped_to_cargo_out_dir() {
        let out_dir = Path::new("cargo-out");

        assert_eq!(release_dist(out_dir), out_dir.join("frontend-dist"));
        assert_ne!(release_dist(out_dir), Path::new("apps/frontend/dist"));
    }
}

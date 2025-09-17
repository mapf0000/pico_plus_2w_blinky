//! This build script copies the `memory.x` file from the crate root into
//! a directory where the linker can always find it at build time.
//! For many projects this is optional, as the linker always searches the
//! project root directory -- wherever `Cargo.toml` is. However, if you
//! are using a workspace or have a more complicated build setup, this
//! build script becomes required. Additionally, by requesting that
//! Cargo re-run the build script whenever `memory.x` is changed,
//! updating `memory.x` ensures a rebuild of the application with the
//! new memory settings.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // Re-run build script when relevant env vars change
    println!("cargo:rerun-if-env-changed=PICO_SKIP_FRONTEND");
    println!("cargo:rerun-if-env-changed=PICO_REQUIRE_FRONTEND");
    println!("cargo:rerun-if-env-changed=PICO_FRONTEND_MODE");
    println!("cargo:rerun-if-env-changed=PICO_WASM_WARN_BYTES");
    println!("cargo:rerun-if-env-changed=PICO_WASM_MAX_BYTES");
    // Put the linker script somewhere the linker can find it
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rustc-link-search={}", out.display());

    // The file `memory.x` is loaded by cortex-m-rt's `link.x` script, which
    // is what we specify in `.cargo/config.toml` for Arm builds
    let memory_x = include_bytes!("memory.x");
    let mut f = File::create(out.join("memory.x")).unwrap();
    f.write_all(memory_x).unwrap();
    println!("cargo:rerun-if-changed=memory.x");

    println!("cargo:rerun-if-changed=build.rs");

    // Also (attempt to) build and embed the WebAssembly frontend via Trunk.
    // This allows serving the compiled frontend directly from the Pico HTTP server.
    if std::env::var_os("PICO_SKIP_FRONTEND").is_some() {
        println!("cargo:warning=Skipping frontend build due to PICO_SKIP_FRONTEND being set");
        write_frontend_stub(&out).expect("failed to write frontend stub");
        return;
    }

    // Only attempt when the `frontend` directory exists (workspace member).
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let frontend_dir = manifest_dir.join("frontend");
    if !frontend_dir.exists() {
        // Still write a stub so the firmware compiles without the frontend.
        write_frontend_stub(&out).expect("failed to write frontend stub");
        return;
    }

    // Ensure changes in the frontend cause rebuild of this crate.
    println!("cargo:rerun-if-changed=frontend/Cargo.toml");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/ui");
    println!("cargo:rerun-if-changed=frontend/Trunk.toml");

    // Decide how to obtain frontend assets.
    // Modes:
    //  - default: on embedded release builds, try building with trunk; on host builds, prefer existing dist or stub
    //  - PICO_FRONTEND_MODE=dist: skip trunk, use existing frontend/dist
    //  - PICO_FRONTEND_MODE=trunk: force trunk build
    let fe_mode = std::env::var("PICO_FRONTEND_MODE").unwrap_or_default();
    let force_trunk = fe_mode.eq_ignore_ascii_case("trunk");
    let use_dist_only = fe_mode.eq_ignore_ascii_case("dist");

    // Determine current build profile/target to decide when a real frontend is needed.
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_default();
    let is_embedded_release = profile == "release" && target.starts_with("thumb");

    // Always ensure a stub exists so host checks work even if embedding fails.
    write_frontend_stub(&out).expect("failed to write frontend stub");
    let dist_present = frontend_dir.join("dist").exists();
    let trunk_ok = if use_dist_only {
        println!("cargo:warning=frontend: using existing dist (PICO_FRONTEND_MODE=dist)");
        false
    } else if dist_present && !force_trunk {
        println!("cargo:warning=frontend: found existing dist; skipping trunk build");
        false
    } else if force_trunk || is_embedded_release {
        if force_trunk {
            println!("cargo:warning=frontend: forcing trunk build (PICO_FRONTEND_MODE=trunk)");
        } else {
            println!("cargo:warning=frontend: embedded release detected; attempting trunk build --release");
        }
        run_trunk_build(&frontend_dir)
    } else {
        // Host builds (including `cargo run` on dev machine) default to not running Trunk
        // to avoid long WASM builds or hangs. We will try to use frontend/dist if present
        // and otherwise fall back to the stub.
        println!("cargo:warning=frontend: host build; skipping trunk. Set PICO_FRONTEND_MODE=trunk to force.");
        false
    };

    // After building (or if dist already available), locate the built JS and WASM, and
    // generate stable copies + an include-able Rust file.
    // For release embedded builds, require a real frontend by default.
    let require_frontend = if std::env::var_os("PICO_REQUIRE_FRONTEND").is_some() {
        true
    } else {
        // Default: require frontend when building release for thumb MCUs.
        is_embedded_release
    };

    match generate_embedded_frontend(&frontend_dir, &out, trunk_ok) {
        Ok(()) => {}
        Err(e) => {
            if require_frontend {
                eprintln!(
                    "error: frontend embedding failed: {e}\n       hint: install trunk (cargo install trunk) and add target wasm32-unknown-unknown"
                );
                std::process::exit(1);
            } else {
                // Fall back to stub on failure; this keeps host-target checks working.
                println!("cargo:warning=frontend embedding skipped: {e}");
                println!("cargo:warning=hint: install trunk (cargo install trunk) and add wasm32-unknown-unknown target");
            }
        }
    }
}

fn run_trunk_build(frontend_dir: &Path) -> bool {
    // Detect presence of trunk
    let trunk_present = Command::new("sh")
        .arg("-c")
        .arg("command -v trunk >/dev/null 2>&1")
        .current_dir(frontend_dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !trunk_present {
        println!("cargo:warning=Trunk not found; attempting to use existing frontend/dist if available");
        return false;
    }

    let status = Command::new("trunk")
        .arg("build")
        .arg("--release")
        .current_dir(frontend_dir)
        .status();
    match status {
        Ok(s) if s.success() => true,
        Ok(s) => {
            println!("cargo:warning=trunk build failed with status: {s}");
            false
        }
        Err(e) => {
            println!("cargo:warning=failed to run trunk: {e}");
            false
        }
    }
}

fn generate_embedded_frontend(frontend_dir: &Path, out_dir: &Path, had_trunk: bool) -> Result<(), String> {
    let dist = frontend_dir.join("dist");
    if !dist.exists() {
        return Err(format!(
            "frontend/dist missing at {} (trunk: {had_trunk})",
            dist.display()
        ));
    }

    // Locate the built JS and WASM assets
    let mut js_file: Option<PathBuf> = None;
    let mut wasm_file: Option<PathBuf> = None;
    for entry in fs::read_dir(&dist).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            match ext {
                "js" if js_file.is_none() => js_file = Some(path),
                "wasm" if wasm_file.is_none() => wasm_file = Some(path),
                _ => {}
            }
        }
    }
    let js_file = js_file.ok_or_else(|| "could not find built .js in frontend/dist".to_string())?;
    let wasm_file = wasm_file.ok_or_else(|| "could not find built .wasm in frontend/dist".to_string())?;

    // Read and rewrite index.html to reference stable paths under /ui
    let index_src = dist.join("index.html");
    let mut index_contents = String::new();
    File::open(&index_src)
        .map_err(|e| format!("open {}: {e}", index_src.display()))?
        .read_to_string(&mut index_contents)
        .map_err(|e| format!("read {}: {e}", index_src.display()))?;

    // Replace wasm module path in init({...}) and any link rel=modulepreload href
    let js_name = js_file
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid js filename".to_string())?;
    let wasm_name = wasm_file
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid wasm filename".to_string())?;

    // Most Trunk templates include `init({ module_or_path: '/<name>_bg.wasm' })`
    index_contents = index_contents.replace(wasm_name, "ui/app.wasm");
    index_contents = index_contents.replace(js_name, "ui/app.js");
    // Also handle absolute "/..." occurrences
    index_contents = index_contents.replace(&format!("/{}", wasm_name), "/ui/app.wasm");
    index_contents = index_contents.replace(&format!("/{}", js_name), "/ui/app.js");
    // And make style/spider relative to /ui as well for self-contained serving.
    // Note: Avoid creating duplicated prefixes like "/ui/ui/style.css" when the
    // source already points to "/ui/style.css" by collapsing any accidental
    // duplications after replacement.
    index_contents = index_contents.replace("/style.css", "/ui/style.css");
    index_contents = index_contents.replace("/spider.svg", "/ui/spider.svg");
    // Collapse any accidental double "/ui/" (e.g., "/ui/ui/style.css") that can
    // occur if the original already referenced "/ui/...".
    if index_contents.contains("/ui/ui/") {
        index_contents = index_contents.replace("/ui/ui/", "/ui/");
    }

    // Write stable copies into OUT_DIR where we'll include from
    let index_out = out_dir.join("frontend_index.html");
    let js_out = out_dir.join("frontend_app.js");
    let wasm_out = out_dir.join("frontend_app.wasm");
    fs::write(&index_out, index_contents).map_err(|e| format!("write {}: {e}", index_out.display()))?;
    fs::copy(&js_file, &js_out).map_err(|e| format!("copy js: {e}"))?;
    fs::copy(&wasm_file, &wasm_out).map_err(|e| format!("copy wasm: {e}"))?;

    // Copy CSS and SVG from Trunk dist (added via copy-file in frontend/index.html)
    let style_out = out_dir.join("frontend_style.css");
    let spider_out = out_dir.join("frontend_spider.svg");
    let css_dist = dist.join("ui/style.css");
    let svg_dist = dist.join("ui/spider.svg");
    if css_dist.exists() {
        fs::copy(&css_dist, &style_out).map_err(|e| format!("copy dist style.css: {e}"))?;
    }
    if svg_dist.exists() {
        fs::copy(&svg_dist, &spider_out).map_err(|e| format!("copy dist spider.svg: {e}"))?;
    }

    // Warn if the WASM is large. Default threshold 1.2 MiB, override with PICO_WASM_WARN_BYTES.
    let wasm_size = fs::metadata(&wasm_file)
        .map_err(|e| format!("metadata for {}: {e}", wasm_file.display()))?
        .len();
    let warn_bytes: u64 = std::env::var("PICO_WASM_WARN_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_200_000);
    if wasm_size > warn_bytes {
        println!(
            "cargo:warning=frontend WASM size {} bytes exceeds {}. Consider size optimizations (opt-level=\"z\", lto, codegen-units=1) or reducing deps.",
            wasm_size, warn_bytes
        );
    }

    // Optional CI guard: fail the build if PICO_WASM_MAX_BYTES is set and exceeded.
    if let Ok(val) = std::env::var("PICO_WASM_MAX_BYTES") {
        if let Ok(max) = val.parse::<u64>() {
            if wasm_size > max {
                eprintln!(
                    "error: frontend WASM size {} exceeds PICO_WASM_MAX_BYTES={} bytes",
                    wasm_size, max
                );
                std::process::exit(1);
            }
        }
    }

    // Generate a small Rust module we can include from the firmware to serve these assets.
    let gen_rs = out_dir.join("frontend_static.rs");
    let mut f = File::create(&gen_rs).map_err(|e| format!("create {}: {e}", gen_rs.display()))?;
    writeln!(
        f,
        "pub static INDEX_HTML: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/frontend_index.html\"));"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        f,
        "pub static APP_JS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_app.js\"));"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        f,
        "pub static APP_WASM: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/frontend_app.wasm\"));"
    )
    .map_err(|e| e.to_string())?;
    if style_out.exists() {
        writeln!(
            f,
            "pub static STYLE_CSS: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/frontend_style.css\"));"
        ).map_err(|e| e.to_string())?;
    } else {
        writeln!(f, "pub static STYLE_CSS: &str = \"\";").map_err(|e| e.to_string())?;
    }
    if spider_out.exists() {
        writeln!(
            f,
            "pub static SPIDER_SVG: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/frontend_spider.svg\"));"
        ).map_err(|e| e.to_string())?;
    } else {
        writeln!(f, "pub static SPIDER_SVG: &str = \"\";").map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn write_frontend_stub(out_dir: &Path) -> Result<(), String> {
    let gen_rs = out_dir.join("frontend_static.rs");
    let mut f = File::create(&gen_rs).map_err(|e| format!("create {}: {e}", gen_rs.display()))?;
    let html = "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Pico Frontend</title></head><body><p>Frontend not built. Install Trunk and run cargo build again.</p></body></html>\n";
    writeln!(f, "pub static INDEX_HTML: &str = r#\"{}\"#;", html).map_err(|e| e.to_string())?;
    writeln!(
        f,
        "pub static APP_JS: &[u8] = &[];\n"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        f,
        "pub static APP_WASM: &[u8] = &[];\n"
    )
    .map_err(|e| e.to_string())?;
    // Provide empty placeholders so firmware can serve optional assets
    // even when the frontend is skipped.
    writeln!(
        f,
        "pub static STYLE_CSS: &str = \"\";\n"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        f,
        "pub static SPIDER_SVG: &str = \"\";\n"
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

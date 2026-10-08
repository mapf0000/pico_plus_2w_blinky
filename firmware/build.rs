fn main() {
    // Portable library tests need neither board linker inputs nor embedded assets.
    if std::env::var_os("CARGO_FEATURE_FIRMWARE").is_none() {
        return;
    }
    if let Err(e) = build_support::run() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}

fn main() {
    println!("cargo:rerun-if-changed=Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let path = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("Info.plist");
        // Give a command-line Mach-O the same Bluetooth privacy declaration as
        // a bundled app, without requiring firmware/browser build tooling.
        println!(
            "cargo:rustc-link-arg-bin=pico-companion=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            path.display()
        );
    }
}

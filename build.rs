
fn main() {
    if let Err(e) = build_support::run() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}

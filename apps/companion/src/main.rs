mod app;
mod backend;
mod smoke;

use clap::Parser;
use companion_core::mock::Scenario;
use eframe::egui;

#[derive(Parser)]
#[command(
    version,
    about = "Native Pico companion feasibility app (mock transport only)"
)]
struct Args {
    /// Use the in-memory device. Hardware Bluetooth is not implemented yet.
    #[arg(long, required = true)]
    mock: bool,
    /// normal, empty, permission-denied, busy, usb-unavailable, incompatible,
    /// timeout, or link-loss
    #[arg(long, default_value = "normal")]
    mock_scenario: Scenario,
    /// Run the normal mock lifecycle without opening a window.
    #[arg(long)]
    self_test: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.self_test {
        if args.mock_scenario != Scenario::Normal {
            return Err("--self-test uses the normal scenario; omit --mock-scenario".into());
        }
        smoke::run()?;
        println!(
            "Mock self-test passed: discovery, connect, control, KBD1 completion, cancel, release, reconnect. No Bluetooth or USB access."
        );
        return Ok(());
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 790.0])
            .with_min_inner_size([480.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Pico Companion — Mock",
        options,
        Box::new(move |cc| Ok(Box::new(app::CompanionApp::new(cc, args.mock_scenario)?))),
    )?;
    Ok(())
}

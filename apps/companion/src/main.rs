mod app;
mod backend;
mod ble;
mod smoke;

use clap::Parser;
use companion_core::mock::Scenario;
use eframe::egui;

#[derive(Parser)]
#[command(
    version,
    about = "Native Pico companion feasibility app (mock or read-only Bluetooth)"
)]
struct Args {
    /// Use an in-memory device.
    #[arg(long, required_unless_present = "ble", conflicts_with = "ble")]
    mock: bool,
    /// Discover and read a Pico running opt-in BLE firmware. No control writes.
    #[arg(long)]
    ble: bool,
    /// normal, empty, permission-denied, busy, usb-unavailable, incompatible,
    /// timeout, or link-loss
    #[arg(long, default_value = "normal")]
    mock_scenario: Scenario,
    /// Run a headless mock lifecycle or real BLE discovery/status/reconnect check.
    #[arg(long)]
    self_test: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let mode = if args.ble {
        backend::Mode::Ble
    } else {
        backend::Mode::Mock(args.mock_scenario)
    };
    if args.self_test {
        if args.ble {
            return ble_smoke();
        }
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
        if args.ble {
            "Pico Companion — Bluetooth"
        } else {
            "Pico Companion — Mock"
        },
        options,
        Box::new(move |cc| Ok(Box::new(app::CompanionApp::new(cc, mode)?))),
    )?;
    Ok(())
}

fn ble_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use companion_core::{Action, Connection};
    use std::{
        thread,
        time::{Duration, Instant},
    };
    enum Phase {
        Scan,
        Connect,
        Samples(usize),
        Disconnect,
        Reconnect,
    }
    let backend = backend::Backend::start(backend::Mode::Ble, egui::Context::default())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut phase = Phase::Scan;
    let mut revision = 0;
    let mut device_id = None;
    let mut initial_uptime = None;
    loop {
        let snapshot = backend.snapshots.borrow().clone();
        if let Some(error) = snapshot.last_error {
            return Err(error.message().into());
        }
        if snapshot.revision != revision {
            revision = snapshot.revision;
            match phase {
                Phase::Scan
                    if snapshot.connection == Connection::Disconnected && !snapshot.pending =>
                {
                    let device = snapshot
                        .devices
                        .first()
                        .ok_or("No Pico BLE devices found")?;
                    device_id = Some(device.id.clone());
                    backend.send(snapshot.epoch, Action::Connect(device.id.clone()))?;
                    phase = Phase::Connect;
                }
                Phase::Connect if snapshot.connection == Connection::Connected => {
                    phase = Phase::Samples(0);
                }
                Phase::Samples(count)
                    if snapshot.connection == Connection::Connected && !snapshot.pending =>
                {
                    let status = snapshot
                        .capabilities
                        .as_ref()
                        .ok_or("Missing capabilities")?
                        .status;
                    let uptime = status.uptime_secs.ok_or("Missing device uptime")?;
                    let start = *initial_uptime.get_or_insert(uptime);
                    if uptime < start {
                        return Err("Device restarted during BLE smoke".into());
                    }
                    if count >= 2 {
                        if uptime == start {
                            return Err("Device uptime did not advance".into());
                        }
                        println!(
                            "BLE status: uptime={uptime}s USB enabled={} ready={} agent={}; latest read={}ms",
                            status.usb_enabled,
                            status.usb_ready,
                            status.host_agent_present,
                            snapshot.last_rtt.unwrap_or_default().as_millis()
                        );
                        backend.send(snapshot.epoch, Action::Disconnect)?;
                        phase = Phase::Disconnect;
                    } else {
                        phase = Phase::Samples(count + 1);
                    }
                }
                Phase::Disconnect if snapshot.connection == Connection::Disconnected => {
                    backend.send(
                        snapshot.epoch,
                        Action::Connect(device_id.clone().ok_or("Missing device")?),
                    )?;
                    phase = Phase::Reconnect;
                }
                Phase::Reconnect if snapshot.connection == Connection::Connected => {
                    println!(
                        "BLE smoke passed: discovery, info, three live status reads, disconnect and reconnect. No control writes."
                    );
                    return Ok(());
                }
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err("BLE smoke timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

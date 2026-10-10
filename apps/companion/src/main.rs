mod app;
mod backend;
mod ble;
mod profile;
mod smoke;

use clap::Parser;
use companion_core::mock::Scenario;
use eframe::egui;

#[derive(Parser)]
#[command(
    version,
    about = "Native Pico companion feasibility app (mock or Bluetooth control)"
)]
struct Args {
    /// Private per-device provisioning profile; required to acquire real BLE control.
    #[arg(long, conflicts_with_all = ["mock", "create_profile"])]
    profile: Option<std::path::PathBuf>,
    /// Generate a private profile and matching .flash.bin; exits without device access.
    #[arg(long, conflicts_with_all = ["mock", "ble", "self_test"])]
    create_profile: Option<std::path::PathBuf>,
    /// Use an in-memory device.
    #[arg(long, conflicts_with = "ble")]
    mock: bool,
    /// Connect to a Pico over Bluetooth.
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
    let mut args = Args::parse();
    if let Some(path) = &args.create_profile {
        let flash = profile::create(path)?;
        println!(
            "Created private profile {} and provisioning image {} (flash address {:#010x}). Keep both private and back up the profile securely.",
            path.display(),
            flash.display(),
            ble_session::provisioning::FLASH_ADDRESS
        );
        return Ok(());
    }
    let profile = args.profile.as_deref().map(profile::load).transpose()?;
    args.ble = !args.mock;
    let mode = if args.ble {
        backend::Mode::Ble
    } else {
        backend::Mode::Mock(args.mock_scenario)
    };
    if args.self_test {
        if args.ble {
            return ble_smoke(profile);
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
        Box::new(move |cc| Ok(Box::new(app::CompanionApp::new(cc, mode, profile)?))),
    )?;
    Ok(())
}

fn ble_smoke(
    profile: Option<ble_session::provisioning::Profile>,
) -> Result<(), Box<dyn std::error::Error>> {
    use companion_core::{Action, Connection};
    use std::{
        thread,
        time::{Duration, Instant},
    };
    enum Phase {
        Scan,
        Connect,
        Acquire,
        Samples(usize),
        Release,
        ReAcquire,
        ReRelease,
        Disconnect,
        Reconnect,
    }
    let authenticated = profile.is_some();
    let backend = backend::Backend::start_with_profile(
        backend::Mode::Ble,
        profile,
        egui::Context::default(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(60);
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
                    if authenticated {
                        backend.send(snapshot.epoch, Action::Acquire)?;
                        phase = Phase::Acquire;
                    } else {
                        phase = Phase::Samples(0);
                    }
                }
                Phase::Acquire if snapshot.control_acquired && !snapshot.pending => {
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
                        if authenticated {
                            backend.send(snapshot.epoch, Action::Release)?;
                            phase = Phase::Release;
                        } else {
                            backend.send(snapshot.epoch, Action::Disconnect)?;
                            phase = Phase::Disconnect;
                        }
                    } else {
                        phase = Phase::Samples(count + 1);
                    }
                }
                Phase::Release if !snapshot.control_acquired && !snapshot.pending => {
                    backend.send(snapshot.epoch, Action::Disconnect)?;
                    phase = Phase::Disconnect;
                }
                Phase::Disconnect if snapshot.connection == Connection::Disconnected => {
                    backend.send(
                        snapshot.epoch,
                        Action::Connect(device_id.clone().ok_or("Missing device")?),
                    )?;
                    phase = Phase::Reconnect;
                }
                Phase::Reconnect if snapshot.connection == Connection::Connected => {
                    if authenticated {
                        backend.send(snapshot.epoch, Action::Acquire)?;
                        phase = Phase::ReAcquire;
                    } else {
                        println!(
                            "Public BLE smoke passed: discovery, live status, disconnect and reconnect. No authentication or control writes."
                        );
                        return Ok(());
                    }
                }
                Phase::ReAcquire if snapshot.control_acquired && !snapshot.pending => {
                    backend.send(snapshot.epoch, Action::Release)?;
                    phase = Phase::ReRelease;
                }
                Phase::ReRelease if !snapshot.control_acquired && !snapshot.pending => {
                    println!(
                        "Authenticated BLE smoke passed: fresh sessions, acquire/release, encrypted live status and reconnect. No Pico interaction or HID effects."
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

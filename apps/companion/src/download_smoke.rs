//! Explicit headless board acceptance. Writes only the chosen local destination.
use crate::backend::{Backend, Mode};
use companion_core::{
    Action, Connection,
    download::{Command, Session, State},
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
pub fn run(
    profile: ble_session::provisioning::Profile,
    path: String,
    destination: PathBuf,
    cancel_after: Option<u64>,
    default_path: bool,
    restart_after_cancel: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let backend =
        Backend::start_with_profile(Mode::Ble, Some(profile), eframe::egui::Context::default())?;
    let began = Instant::now();
    let mut phase = 0;
    let mut download_at = None;
    let mut last_progress = None;
    loop {
        let s = backend.snapshots.borrow().clone();
        if let Some(error) = s.last_error {
            return Err(format!(
                "{}; receiver: {}",
                error.message(),
                s.downloads.error.as_deref().unwrap_or("no receiver error")
            )
            .into());
        }
        if let Some(item) = s.downloads.items.back() {
            let progress = (item.state, item.received);
            if last_progress != Some(progress) {
                println!(
                    "Download progress: {:?}, {} / {} bytes",
                    item.state, item.received, item.total
                );
                last_progress = Some(progress);
            }
        }
        match phase {
            0 if s.epoch != 0 && s.connection == Connection::Disconnected && !s.pending => {
                let device = s.devices.first().ok_or("Pico not found")?;
                println!("Download check: connect");
                backend.send(s.epoch, Action::Connect(device.id.clone()))?;
                phase = 1;
            }
            1 if s.connection == Connection::Connected && !s.pending => {
                if !s
                    .capabilities
                    .as_ref()
                    .is_some_and(|c| c.downloads_supported)
                {
                    return Err("Firmware does not support downloads".into());
                }
                println!("Download check: acquire");
                backend.send(s.epoch, Action::Acquire)?;
                phase = 2;
            }
            2 if s.control_acquired && !s.pending => {
                println!("Download check: host session");
                backend.send(s.epoch, Action::Download(Command::ConnectSession))?;
                phase = 3;
            }
            3 if s.downloads.session == Session::Ready && !s.pending && default_path => {
                println!("Download check: set default");
                backend.send(s.epoch, Action::Download(Command::SetDefault(path.clone())))?;
                phase = 30;
            }
            3 if s.downloads.session == Session::Ready && !s.pending => {
                println!("Download check: queue");
                backend.send(
                    s.epoch,
                    Action::Download(Command::Queue {
                        path: path.clone(),
                        destination: destination.clone(),
                    }),
                )?;
                download_at = Some(Instant::now());
                phase = 4;
            }
            30 if !s.pending => {
                println!("Download check: queue default");
                backend.send(
                    s.epoch,
                    Action::Download(Command::QueueDefault {
                        destination: destination.clone(),
                    }),
                )?;
                download_at = Some(Instant::now());
                phase = 4;
            }
            4 => {
                if s.downloads.items.iter().any(|i| i.state == State::Saved) {
                    let item = s
                        .downloads
                        .items
                        .iter()
                        .find(|i| i.state == State::Saved)
                        .unwrap();
                    println!(
                        "Encrypted download saved and verified: {} bytes in {:.3}s, {} retries",
                        item.received,
                        download_at.unwrap().elapsed().as_secs_f64(),
                        item.retries
                    );
                    phase = 6;
                } else if let Some(ms) = cancel_after
                    && download_at.unwrap().elapsed() >= Duration::from_millis(ms)
                {
                    println!("Download check: cancel");
                    backend.send(s.epoch, Action::Download(Command::Cancel))?;
                    phase = 5;
                }
            }
            5 if !s.downloads.active() && !s.pending => {
                if destination.exists() {
                    return Err("Cancellation unexpectedly saved a file".into());
                }
                println!("Download cancellation passed; no destination was saved.");
                if restart_after_cancel {
                    println!("Download check: restart host session on the same BLE connection");
                    backend.send(s.epoch, Action::Download(Command::ConnectSession))?;
                    phase = 9;
                } else {
                    phase = 6;
                }
            }
            9 if s.downloads.session == Session::Ready && !s.pending => {
                backend.send(
                    s.epoch,
                    Action::Download(Command::Queue {
                        path: path.clone(),
                        destination: destination.clone(),
                    }),
                )?;
                download_at = Some(Instant::now());
                phase = 10;
            }
            10 if download_at.unwrap().elapsed()
                >= Duration::from_millis(cancel_after.unwrap()) =>
            {
                backend.send(s.epoch, Action::Download(Command::Cancel))?;
                phase = 11;
            }
            11 if !s.downloads.active() && !s.pending => {
                if destination.exists() {
                    return Err("Restarted cancellation unexpectedly saved a file".into());
                }
                println!("Same-link host session restart and second cancellation passed.");
                phase = 6;
            }
            6 if !s.pending => {
                backend.send(s.epoch, Action::Release)?;
                phase = 7;
            }
            7 if !s.control_acquired && !s.pending => {
                backend.send(s.epoch, Action::Disconnect)?;
                phase = 8;
            }
            8 if s.connection == Connection::Disconnected => return Ok(()),
            _ => {}
        }
        if (phase == 10 || (matches!(phase, 3 | 4 | 30) && cancel_after.is_none()))
            && s.downloads.session == Session::Failed
        {
            return Err(s.downloads.error.unwrap_or("Download failed".into()).into());
        }
        if began.elapsed() > Duration::from_secs(180) {
            return Err("Download acceptance timed out".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

//! Explicit, reversible hardware check. Ordinary --self-test never edits flash.
use crate::backend::{Backend, Mode};
use companion_core::{Action, Connection, IdentityUpdate, Snapshot, UsbIdentity};
use std::{
    error::Error,
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn wait(backend: &Backend, label: &str, done: impl Fn(&Snapshot) -> bool) -> Result<Snapshot> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = backend.snapshots.borrow().clone();
        if let Some(error) = snapshot.last_error {
            return Err(error.message().into());
        }
        if done(&snapshot) {
            return Ok(snapshot);
        }
        if Instant::now() >= deadline {
            return Err(format!("Identity smoke timed out: {label}").into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn send(backend: &Backend, action: Action) -> Result<()> {
    let snapshot = backend.snapshots.borrow().clone();
    backend.send(snapshot.epoch, action)?;
    // Observe dispatch before interpreting a retained error or an already-satisfied state.
    let deadline = Instant::now() + Duration::from_secs(5);
    while backend.snapshots.borrow().revision == snapshot.revision {
        if Instant::now() >= deadline {
            return Err("Identity smoke command was not dispatched".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn acquired(backend: &Backend) -> Result<Snapshot> {
    send(backend, Action::Acquire)?;
    wait(backend, "acquire and read identity", |s| {
        s.can_read_identity()
            && s.capabilities
                .as_ref()
                .is_some_and(|c| c.identity.is_some())
    })
}
fn usb(backend: &Backend, enabled: bool) -> Result<()> {
    let snapshot = backend.snapshots.borrow().clone();
    if !snapshot.pending
        && snapshot
            .capabilities
            .as_ref()
            .is_some_and(|c| c.status.usb_enabled == enabled && c.status.usb_ready == enabled)
    {
        return Ok(());
    }
    send(backend, Action::SetUsbEnabled(enabled))?;
    wait(backend, "USB toggle", |s| {
        !s.pending
            && s.capabilities
                .as_ref()
                .is_some_and(|c| c.status.usb_enabled == enabled && c.status.usb_ready == enabled)
    })?;
    Ok(())
}
fn read(backend: &Backend, expected: &UsbIdentity) -> Result<()> {
    let revision = backend.snapshots.borrow().identity_revision;
    send(backend, Action::ReadIdentity)?;
    let snapshot = wait(backend, "identity read", |s| {
        s.identity_revision > revision && s.identity_update == IdentityUpdate::Loaded
    })?;
    if snapshot
        .capabilities
        .as_ref()
        .and_then(|c| c.identity.as_ref())
        != Some(expected)
    {
        return Err("Identity readback mismatch".into());
    }
    Ok(())
}
fn save(backend: &Backend, identity: &UsbIdentity) -> Result<()> {
    let revision = backend.snapshots.borrow().identity_revision;
    send(backend, Action::SaveIdentity(identity.clone()))?;
    wait(backend, "verified save", |s| {
        s.identity_revision > revision && s.identity_update == IdentityUpdate::Saved
    })?;
    read(backend, identity)
}

pub fn run(profile: ble_session::provisioning::Profile) -> Result<()> {
    let backend =
        Backend::start_with_profile(Mode::Ble, Some(profile), eframe::egui::Context::default())?;
    send(&backend, Action::Scan)?;
    let snapshot = wait(&backend, "discovery", |s| {
        s.connection == Connection::Disconnected && !s.devices.is_empty()
    })?;
    let id = snapshot.devices[0].id.clone();
    send(&backend, Action::Connect(id.clone()))?;
    wait(&backend, "connect", |s| {
        s.connection == Connection::Connected
    })?;
    let snapshot = acquired(&backend)?;
    let caps = snapshot
        .capabilities
        .as_ref()
        .ok_or("Missing capabilities")?;
    let original = caps
        .identity
        .clone()
        .ok_or("Firmware does not support USB identity")?;
    let enabled = caps.status.usb_enabled;
    let temporary =
        UsbIdentity::new("Pico BLE test", "Identity test 20261011").map_err(|e| e.message())?;
    let test = (|| -> Result<()> {
        usb(&backend, false)?;
        println!("USB disabled. Holding 5 seconds for host removal inspection.");
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            thread::sleep(Duration::from_millis(50));
        }
        save(&backend, &temporary)?;
        usb(&backend, true)?;
        println!(
            "Temporary USB identity applied: Pico BLE test / Identity test 20261011. Holding 10 seconds for descriptor inspection."
        );
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            thread::sleep(Duration::from_millis(50));
        }
        // Repeat stop/start to catch stale shutdown acknowledgement reuse.
        for _ in 0..2 {
            usb(&backend, false)?;
            usb(&backend, true)?;
        }
        send(&backend, Action::Disconnect)?;
        wait(&backend, "disconnect", |s| {
            s.connection == Connection::Disconnected
        })?;
        send(&backend, Action::Connect(id))?;
        wait(&backend, "reconnect", |s| {
            s.connection == Connection::Connected
        })?;
        acquired(&backend)?;
        read(&backend, &temporary)?;
        Ok(())
    })();
    // Always attempt to restore the captured values and original USB state.
    let restore = (|| -> Result<()> {
        usb(&backend, false)?;
        save(&backend, &original)?;
        usb(&backend, enabled)?;
        Ok(())
    })();
    if let Err(error) = restore {
        return Err(format!("Identity smoke restoration failed: {error}. Reload identity and restore the original names before further use.").into());
    }
    test?;
    send(&backend, Action::Release)?;
    wait(&backend, "release", |s| !s.control_acquired && !s.pending)?;
    println!(
        "Identity smoke passed: verified save/readback, three USB stop/start cycles, fresh reconnect, original names and USB state restored. Independent power-cycle acceptance remains separate."
    );
    Ok(())
}

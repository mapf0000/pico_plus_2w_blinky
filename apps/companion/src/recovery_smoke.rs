//! Explicit local USB-agent/BLE recovery acceptance using private disposable files.
use crate::backend::{Backend, Mode};
use companion_core::{
    Action, Connection, Snapshot,
    download::{Command, Session, State},
};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Harness {
    backend: Backend,
}
impl Harness {
    fn snapshot(&self) -> Snapshot {
        self.backend.snapshots.borrow().clone()
    }
    fn send(&self, action: Action) -> Result<()> {
        let epoch = self.snapshot().epoch;
        self.backend.send(epoch, action).map_err(Into::into)
    }
    fn wait(&self, phase: &str, expected: impl Fn(&Snapshot) -> bool) -> Result<Snapshot> {
        println!("Recovery check: {phase}");
        let deadline = Instant::now() + Duration::from_secs(150);
        loop {
            let s = self.snapshot();
            if expected(&s) {
                return Ok(s);
            }
            if Instant::now() >= deadline {
                return Err(format!("Recovery timed out: {phase}; connection {:?}, pending {}, session {:?}, active {}, device error {:?}, receiver error {}",
                    s.connection, s.pending, s.downloads.session, s.downloads.active(), s.last_error,
                    s.downloads.error.as_deref().unwrap_or("none")).into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn acquire(&self) -> Result<()> {
        if self.snapshot().control_acquired && !self.snapshot().pending {
            return Ok(());
        }
        self.send(Action::Acquire)?;
        self.wait("acquire", |s| s.control_acquired && !s.pending)?;
        Ok(())
    }
    fn reconnect(&self) -> Result<()> {
        self.send(Action::Reconnect)?;
        let s = self.wait("fresh BLE reconnect", |s| {
            s.connection == Connection::Connected && !s.pending
        })?;
        if s.control_acquired || s.downloads.active() {
            return Err("Reconnect silently acquired or resumed downloads".into());
        }
        self.acquire()
    }
    fn session(&self) -> Result<()> {
        self.wait("host agent available", |s| {
            s.control_acquired
                && s.capabilities
                    .as_ref()
                    .is_some_and(|c| c.status.host_agent_present)
        })?;
        if self.snapshot().downloads.session == Session::Ready {
            self.send(Action::Download(Command::Cancel))?;
            self.wait("session cancellation barrier", |s| {
                matches!(s.downloads.session, Session::Idle | Session::Failed)
                    && !s.downloads.active()
                    && !s.pending
            })?;
        }
        self.send(Action::Download(Command::ConnectSession))?;
        self.wait("host session", |s| {
            s.downloads.session == Session::Ready && !s.pending
        })?;
        Ok(())
    }
    fn queue(&self, source: &Path, destination: &Path) -> Result<()> {
        let command = Command::Queue {
            path: source.to_str().ok_or("Non-UTF8 fixture path")?.into(),
            destination: destination.into(),
        };
        if !command.valid() {
            return Err("Fixture path exceeds protocol bounds".into());
        }
        self.send(Action::Download(command))?;
        self.wait("queue admission", |s| {
            s.downloads
                .items
                .iter()
                .any(|i| i.destination == destination)
        })?;
        Ok(())
    }
    fn receiving(&self, destination: &Path) -> Result<()> {
        let s = self.wait("first authenticated/disk-credited data", |s| {
            matches!(s.connection, Connection::Failed(_))
                || s.downloads.items.iter().any(|i| {
                    i.destination == destination
                        && (i.state == State::Receiving && i.received > 0
                            || matches!(i.state, State::Failed | State::Aborted | State::Saved))
                })
        })?;
        if !s
            .downloads
            .items
            .iter()
            .any(|i| i.destination == destination && i.state == State::Receiving && i.received > 0)
        {
            return Err(format!(
                "Transfer ended before receiving data: connection {:?}, session {:?}, receiver error {}",
                s.connection,
                s.downloads.session,
                s.downloads.error.as_deref().unwrap_or("none")
            )
            .into());
        }
        Ok(())
    }
    fn saved(&self, source: &Path, destination: &Path) -> Result<()> {
        self.wait("verified save/receipt", |s| {
            !s.downloads.active()
                && !s.pending
                && s.downloads
                    .items
                    .iter()
                    .any(|i| i.destination == destination && i.state == State::Saved)
        })?;
        // Sources are created here and bounded below; never read arbitrary files.
        if fs::read(source)? != fs::read(destination)? {
            return Err("Saved fixture differs from source".into());
        }
        Ok(())
    }
    fn cancel(&self, destination: &Path) -> Result<()> {
        self.send(Action::Download(Command::Cancel))?;
        self.wait("cancel cleanup", |s| {
            !s.downloads.active()
                && !s.pending
                && s.downloads
                    .items
                    .iter()
                    .any(|i| i.destination == destination && i.state == State::Aborted)
        })?;
        if destination.exists() {
            return Err("Cancellation published a destination".into());
        }
        Ok(())
    }
    fn usb(&self, enabled: bool) -> Result<()> {
        if self
            .snapshot()
            .capabilities
            .as_ref()
            .is_some_and(|c| c.status.usb_enabled == enabled && (!enabled || c.status.usb_ready))
        {
            return Ok(());
        }
        self.send(Action::SetUsbEnabled(enabled))?;
        self.wait("USB state", |s| {
            !s.pending
                && s.capabilities.as_ref().is_some_and(|c| {
                    c.status.usb_enabled == enabled && (!enabled || c.status.usb_ready)
                })
        })?;
        Ok(())
    }
}

pub fn run(profile: ble_session::provisioning::Profile, parent: PathBuf) -> Result<()> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("download-recovery-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let root = builder.tempdir_in(parent.canonicalize()?)?.keep();
    println!("Recovery fixtures: {}", root.display());
    let source = root.join("source");
    let saved = root.join("received");
    fs::create_dir(&source)?;
    fs::create_dir(&saved)?;
    for (name, size) in [("a.bin", 1025), ("b.bin", 4097), ("long.bin", 32769)] {
        fs::write(
            source.join(name),
            (0..size)
                .map(|i| ((i * 37 + 11) % 256) as u8)
                .collect::<Vec<_>>(),
        )?;
    }
    let h = Harness {
        backend: Backend::start_with_profile(
            Mode::Ble,
            Some(profile),
            eframe::egui::Context::default(),
        )?,
    };
    let mut s = h.wait("discovery", |s| {
        s.epoch != 0
            && !s.pending
            && matches!(
                s.connection,
                Connection::Disconnected | Connection::Failed(_)
            )
    })?;
    // A scan can finish without an advertisement; retry discovery only, never
    // an effect or transfer. Observe the new epoch before accepting completion.
    for _ in 0..2 {
        if !s.devices.is_empty() {
            break;
        }
        let epoch = s.epoch;
        h.send(Action::Scan)?;
        s = h.wait("fresh discovery", |s| {
            s.epoch != epoch
                && !s.pending
                && matches!(
                    s.connection,
                    Connection::Disconnected | Connection::Failed(_)
                )
        })?;
    }
    if s.devices.len() != 1 {
        return Err("Recovery requires exactly one discovered Pico".into());
    }
    h.send(Action::Connect(s.devices[0].id.clone()))?;
    let s = h.wait("connect", |s| {
        s.connection == Connection::Connected && !s.pending
    })?;
    let original_usb = s
        .capabilities
        .as_ref()
        .is_some_and(|c| c.status.usb_enabled);
    if !original_usb {
        return Err(
            "Enable USB and start the local test host agent before recovery acceptance".into(),
        );
    }
    let check = check(&h, &source, &saved);
    // USB restoration is attempted on failure as well, before releasing BLE.
    let restore = (|| -> Result<()> {
        if h.snapshot().connection != Connection::Connected {
            h.reconnect()?;
        }
        if !h.snapshot().control_acquired {
            h.acquire()?;
        }
        h.usb(original_usb)?;
        h.send(Action::Release)?;
        h.wait("release", |s| !s.control_acquired && !s.pending)?;
        h.send(Action::Disconnect)?;
        h.wait("disconnect", |s| s.connection == Connection::Disconnected)?;
        Ok(())
    })();
    match (check, restore) {
        (Err(error), Err(cleanup)) => {
            return Err(format!("{error}; USB/control restoration also failed: {cleanup}").into());
        }
        (Err(error), _) | (_, Err(error)) => return Err(error),
        _ => {}
    }
    println!(
        "Download recovery acceptance passed; USB restored. Fixtures retained privately for inspection."
    );
    Ok(())
}
fn check(h: &Harness, source: &Path, saved: &Path) -> Result<()> {
    h.acquire()?;
    h.session()?;
    let a = source.join("a.bin");
    let b = source.join("b.bin");
    let long = source.join("long.bin");
    let out_a = saved.join("batch-a.bin");
    let out_b = saved.join("batch-b.bin");
    h.queue(&a, &out_a)?;
    h.queue(&b, &out_b)?;
    if !h
        .snapshot()
        .downloads
        .items
        .iter()
        .any(|i| i.destination == out_b && i.state == State::Queued)
    {
        return Err("Did not observe the second file queued behind the first".into());
    }
    h.saved(&a, &out_a)?;
    h.saved(&b, &out_b)?;
    println!("Recovery: two queued files saved byte-for-byte and receipts completed.");

    h.session()?;
    let missing = saved.join("missing.bin");
    h.queue(&source.join("absent.bin"), &missing)?;
    h.wait("missing-source failure", |s| {
        !s.downloads.active()
            && !s.pending
            && s.downloads
                .items
                .iter()
                .any(|i| i.destination == missing && i.state == State::Failed)
    })?;
    if missing.exists() {
        return Err("Missing source unexpectedly saved a destination".into());
    }
    println!("Recovery: missing source failed without a destination.");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        h.session()?;
        let denied = saved.join("unwritable");
        fs::create_dir(&denied)?;
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o0))?;
        let destination = denied.join("result.bin");
        let result = (|| -> Result<()> {
            h.queue(&a, &destination)?;
            h.wait("unwritable destination failure", |s| {
                !s.downloads.active()
                    && !s.pending
                    && s.downloads
                        .items
                        .iter()
                        .any(|i| i.destination == destination && i.state == State::Failed)
            })?;
            Ok(())
        })();
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o700))?;
        result?;
        if destination.exists() {
            return Err("Unwritable destination unexpectedly published a file".into());
        }
        // A fatal record/disk failure deliberately closes BLE rather than
        // risking replay of an uncertain sealed control. Recover explicitly.
        h.wait("fail-closed disk-error disconnect", |s| {
            matches!(
                s.connection,
                Connection::Disconnected | Connection::Failed(_)
            )
        })?;
        h.reconnect()?;
        println!(
            "Recovery: unwritable destination failed without a file; explicit reconnect passed."
        );
    }
    #[cfg(not(unix))]
    println!(
        "Recovery: destination-permission case skipped; requires platform-specific acceptance."
    );

    for index in 0..2 {
        h.session()?;
        let destination = saved.join(format!("cancel-{index}.bin"));
        h.queue(&long, &destination)?;
        h.receiving(&destination)?;
        h.cancel(&destination)?;
        println!(
            "Recovery: cancellation {} after received data cleaned up.",
            index + 1
        );
    }
    h.session()?;
    let disconnected = saved.join("disconnected.bin");
    h.queue(&long, &disconnected)?;
    h.receiving(&disconnected)?;
    h.send(Action::Disconnect)?;
    h.wait("disconnect cleanup", |s| {
        s.connection == Connection::Disconnected && !s.downloads.active()
    })?;
    if disconnected.exists() {
        return Err("Disconnect published an incomplete destination".into());
    }
    h.reconnect()?;
    h.session()?;
    let recovered = saved.join("after-reconnect.bin");
    h.queue(&a, &recovered)?;
    h.saved(&a, &recovered)?;
    println!("Recovery: forced BLE disconnect and explicit fresh-session download passed.");

    h.session()?;
    let detached = saved.join("usb-detached.bin");
    h.queue(&long, &detached)?;
    h.receiving(&detached)?;
    h.usb(false)?;
    h.wait("USB/agent-loss cleanup", |s| !s.downloads.active())?;
    if detached.exists() {
        return Err("USB detach published an incomplete destination".into());
    }
    h.usb(true)?;
    h.wait("agent returned", |s| {
        s.capabilities
            .as_ref()
            .is_some_and(|c| c.status.host_agent_present)
    })?;
    h.session()?;
    let recovered = saved.join("after-usb.bin");
    h.queue(&a, &recovered)?;
    h.saved(&a, &recovered)?;
    println!("Recovery: USB detach/agent loss, re-enumeration and fresh download passed.");
    Ok(())
}

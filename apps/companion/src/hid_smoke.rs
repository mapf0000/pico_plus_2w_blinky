//! Explicit real-device acceptance in a focused, non-editable local key sink.
//! No system chords or Enter are sent. Focus loss cancels/disconnects owned work.
use crate::{
    backend::{Backend, Mode},
    python,
};
use companion_core::{Action, Connection, Job, Outcome, Snapshot};
use eframe::egui;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct Capture {
    down: [u16; 3],
    up: [u16; 3],
    held: [bool; 3],
    shifted_b: bool,
    order: Vec<u8>,
    allow_shift: bool,
    shift_held: [bool; 2],
    shift_down: u8,
    shift_up: u8,
}
impl Capture {
    fn observe(&mut self, events: &[egui::Event]) -> std::result::Result<bool, &'static str> {
        let mut activity = false;
        for event in events {
            if let egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } = event
            {
                activity = true;
                if *repeat || modifiers.ctrl || modifiers.alt || modifiers.command {
                    return Err("Unexpected repeat/system modifier in the capture window");
                }
                let physical = physical_key.unwrap_or(*key);
                if matches!(physical, egui::Key::ShiftLeft | egui::Key::ShiftRight) {
                    if !self.allow_shift {
                        return Err("Unexpected Shift outside the chord phase");
                    }
                    let index = usize::from(physical == egui::Key::ShiftRight);
                    if self.shift_held[index] == *pressed || (*pressed && self.shift_down >= 4) {
                        return Err("Duplicate or excessive Shift edge");
                    }
                    self.shift_held[index] = *pressed;
                    if *pressed {
                        self.shift_down += 1;
                    } else {
                        self.shift_up += 1;
                    }
                    continue;
                }
                let index = match physical {
                    egui::Key::A => 0,
                    egui::Key::B => 1,
                    egui::Key::C => 2,
                    _ => return Err("Unexpected key in capture; do not type during this test"),
                };
                if *pressed {
                    if self.held[index] || self.order.len() == 128 {
                        return Err("Duplicate key-down or capture event bound exceeded");
                    }
                    if modifiers.shift && (index != 1 || !self.allow_shift) {
                        return Err("Unexpected Shift modifier on test letter");
                    }
                    self.order.push(index as u8);
                    self.down[index] += 1;
                    self.held[index] = true;
                    if index == 1 && modifiers.shift {
                        self.shifted_b = true;
                    }
                } else {
                    if !self.held[index] {
                        return Err("Key release without a captured key-down");
                    }
                    self.up[index] += 1;
                    self.held[index] = false;
                }
                if self.down.iter().copied().sum::<u16>() > 128 {
                    return Err("Capture event bound exceeded");
                }
            }
        }
        Ok(activity)
    }
    fn released(&self, shift: bool) -> bool {
        !shift
            && !self.held.iter().any(|held| *held)
            && self.down == self.up
            && !self.shift_held.iter().any(|held| *held)
            && self.shift_down == self.shift_up
    }
}

pub fn run(profile: ble_session::provisioning::Profile) -> Result<()> {
    let outcome = Arc::new(Mutex::new(None));
    let result = outcome.clone();
    eframe::run_native(
        "Pico keyboard acceptance — capture window",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([660.0, 320.0])
                .with_active(true),
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(HidCheck {
                backend: Backend::start_with_profile(
                    Mode::Ble,
                    Some(profile),
                    cc.egui_ctx.clone(),
                )?,
                outcome,
                clock: Instant::now(),
                phase: 0,
                at: Instant::now(),
                capture: Capture::default(),
                armed: false,
                settling: None,
                script_run: 0,
                started: false,
                scan_attempts: 1,
                waiting_scan_epoch: None,
            }))
        }),
    )?;
    result
        .lock()
        .map_err(|_| "HID result lock failed")?
        .take()
        .unwrap_or(Err(
            "Capture window closed before acceptance completed".into()
        ))
        .map_err(Into::into)
}
struct HidCheck {
    backend: Backend,
    outcome: Arc<Mutex<Option<std::result::Result<(), String>>>>,
    clock: Instant,
    phase: u8,
    at: Instant,
    capture: Capture,
    armed: bool,
    settling: Option<Instant>,
    script_run: u64,
    started: bool,
    scan_attempts: u8,
    waiting_scan_epoch: Option<u64>,
}
impl HidCheck {
    fn send(&self, s: &Snapshot, action: Action) -> std::result::Result<(), String> {
        self.backend.send(s.epoch, action).map_err(Into::into)
    }
    fn next(&mut self, phase: u8) {
        println!("HID check phase: {phase}");
        self.phase = phase;
        self.at = Instant::now();
        self.settling = None;
    }
    fn reset(&mut self) {
        self.capture = Capture::default();
        self.settling = None;
    }
    fn settle(&mut self, done: bool) -> bool {
        if !done {
            self.settling = None;
            return false;
        }
        self.settling.get_or_insert_with(Instant::now).elapsed() >= Duration::from_millis(600)
    }
    fn text(&self, s: &Snapshot, count: usize, delay: u32) -> std::result::Result<(), String> {
        self.send(
            s,
            Action::SendText {
                text: "a".repeat(count),
                layout: "win_en-US".into(),
                delay_ms: delay,
            },
        )
    }
    fn step(&mut self, ctx: &egui::Context) -> std::result::Result<bool, String> {
        let s = self.backend.snapshots.borrow().clone();
        let p = self.backend.scripts.borrow().clone();
        let focused = ctx.input(|i| i.focused);
        if !self.started {
            if !focused {
                return Ok(false);
            }
            self.started = true;
            self.clock = Instant::now();
            self.at = Instant::now();
            if matches!(
                s.connection,
                Connection::Disconnected | Connection::Failed(_)
            ) {
                self.waiting_scan_epoch = Some(s.epoch);
                self.send(&s, Action::Scan)?;
                return Ok(false);
            }
        }
        let waiting_scan = self.waiting_scan_epoch == Some(s.epoch);
        if !waiting_scan {
            self.waiting_scan_epoch = None;
        }
        if self.armed && !focused {
            return Err("Capture lost focus; owned work cancelled".into());
        }
        if self.clock.elapsed() > Duration::from_secs(180)
            || self.at.elapsed() > Duration::from_secs(40)
        {
            return Err(format!(
                "HID acceptance timed out at phase {} (connection {:?}, control {}, pending {}, USB {:?}, focused {})",
                self.phase,
                s.connection,
                s.control_acquired,
                s.pending,
                s.capabilities.as_ref().map(|c| c.status),
                focused
            ));
        }
        if waiting_scan {
            return Ok(false);
        }
        if let Some(error) = s.last_error {
            return Err(format!(
                "HID phase {}: {} (connection {:?}, control {}, pending {})",
                self.phase,
                error.message(),
                s.connection,
                s.control_acquired,
                s.pending
            ));
        }
        if p.state == python::State::Faulted {
            return Err(p.error.unwrap_or_else(|| "Python faulted".into()));
        }
        if self.armed {
            let activity = ctx
                .input(|i| self.capture.observe(&i.events))
                .map_err(str::to_owned)?;
            if activity {
                // A down/up pair can arrive in one frame. It must restart the
                // quiet interval even though no key is held at the frame's end.
                self.settling = None;
            }
        }
        let released = self.capture.released(ctx.input(|i| i.modifiers.shift));
        let phase = self.phase;
        match phase {
            0 if s.epoch != 0
                && !s.pending
                && s.connection == Connection::Disconnected
                && s.devices.is_empty() =>
            {
                if self.scan_attempts >= 3 {
                    return Err("No Pico found after three discovery attempts".into());
                }
                self.scan_attempts += 1;
                println!("HID discovery: attempt {} of 3", self.scan_attempts);
                self.waiting_scan_epoch = Some(s.epoch);
                self.send(&s, Action::Scan)?;
                self.at = Instant::now();
            }
            0 if !s.pending
                && s.connection == Connection::Disconnected
                && !s.devices.is_empty() =>
            {
                if s.devices.len() != 1 {
                    return Err("HID test requires exactly one discovered Pico".into());
                }
                self.send(&s, Action::Connect(s.devices[0].id.clone()))?;
                self.next(1);
            }
            1 if s.connection == Connection::Connected && !s.pending => {
                self.send(&s, Action::Acquire)?;
                self.next(2);
            }
            2 if s.can_send() && focused => {
                // A warm-up delay gives the native window time to become the target.
                self.armed = true;
                self.reset();
                self.send(
                    &s,
                    Action::SendText {
                        text: "abc".into(),
                        layout: "win_en-US".into(),
                        delay_ms: 1000,
                    },
                )?;
                self.next(3);
                println!("HID check: manual text to focused capture window");
            }
            3 if self.settle(s.job == Job::Finished(Outcome::Completed) && released) => {
                if self.capture.down != [1, 1, 1]
                    || self.capture.order != [0, 1, 2]
                    || self.capture.shifted_b
                {
                    return Err("Manual text key sequence/count mismatch".into());
                }
                println!("Manual text: three key-down/up pairs and final release passed.");
                self.reset();
                self.capture.allow_shift = true;
                self.backend.start_script(s.epoch, "layout(\"win_en-US\")\ndef main():\n    yield require_usb()\n    yield text(\"a\", 0)\n    yield modtap(\"SHIFT+B\")\n    yield text(\"c\", 0)\n".into()).map_err(str::to_owned)?;
                self.script_run = p.run;
                self.next(4);
            }
            4 if p.run > self.script_run
                && self.settle(p.state == python::State::Completed && released) =>
            {
                if self.capture.down != [1, 1, 1]
                    || self.capture.order != [0, 1, 2]
                    || !self.capture.shifted_b
                    || self.capture.shift_down != 1
                {
                    return Err("Python text/Shift chord capture mismatch".into());
                }
                println!("Python text/Shift chord: three pairs and final modifier release passed.");
                self.reset();
                self.text(&s, 3, 5000)?;
                self.next(5);
            }
            5 if s.job == Job::Running => {
                self.send(&s, Action::Cancel)?;
                self.next(6);
            }
            6 if self.settle(s.job == Job::Finished(Outcome::Cancelled) && released) => {
                if self.capture.down != [0; 3] {
                    return Err("Delayed cancellation emitted a key".into());
                }
                println!("Delay cancellation: no key events passed.");
                self.reset();
                self.text(&s, 100, 1000)?;
                self.next(7);
            }
            7 if self.capture.down[0] > 0 => {
                self.send(&s, Action::Cancel)?;
                self.next(8);
            }
            8 if self.settle(s.job == Job::Finished(Outcome::Cancelled) && released) => {
                if !(1..100).contains(&self.capture.down[0]) || self.capture.down[1..] != [0, 0] {
                    return Err("Typing cancellation did not interrupt output".into());
                }
                println!(
                    "Typing cancellation: {} key pairs, no held keys; completion observed in {}ms (includes 600ms quiet check).",
                    self.capture.down[0],
                    self.at.elapsed().as_millis()
                );
                self.reset();
                self.script_run = p.run;
                self.backend.start_script(s.epoch, "layout(\"win_en-US\")\ndef main():\n    yield require_usb()\n    yield sleep(1000)\n    yield text(\"a\" * 100, 20)\n".into()).map_err(str::to_owned)?;
                self.next(9);
            }
            9 if p.run > self.script_run && self.capture.down[0] > 0 => {
                self.backend.stop_script(p.run).map_err(str::to_owned)?;
                self.next(10);
            }
            10 if self.settle(p.state == python::State::Stopped && !s.job.active() && released) => {
                if !(1..100).contains(&self.capture.down[0]) {
                    return Err("Python Stop did not interrupt output".into());
                }
                println!(
                    "Python Stop: {} key pairs and final release passed.",
                    self.capture.down[0]
                );
                self.reset();
                self.text(&s, 100, 1000)?;
                self.next(11);
            }
            11 if self.capture.down[0] > 0 => {
                self.send(&s, Action::Disconnect)?;
                self.next(12);
            }
            12 if self.settle(s.connection == Connection::Disconnected && released) => {
                if !(1..100).contains(&self.capture.down[0]) {
                    return Err("Disconnect did not interrupt output".into());
                }
                println!(
                    "Disconnect during typing: {} pairs and final release passed.",
                    self.capture.down[0]
                );
                self.reset();
                self.send(&s, Action::Reconnect)?;
                self.next(13);
            }
            13 if s.connection == Connection::Connected && !s.pending => {
                if s.control_acquired || self.capture.down != [0; 3] {
                    return Err("Reconnect reacquired/replayed work".into());
                }
                self.send(&s, Action::Acquire)?;
                self.next(14);
            }
            14 if s.control_acquired && !s.pending => {
                self.send(&s, Action::Release)?;
                self.next(15);
            }
            15 if !s.control_acquired && !s.pending => {
                self.send(&s, Action::Disconnect)?;
                self.next(16);
            }
            16 if s.connection == Connection::Disconnected => {
                println!(
                    "HID acceptance passed: fresh reconnect, explicit reacquire, no replay. Capture is native key events, not raw USB reports."
                );
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
    fn finish(&mut self, ctx: &egui::Context, outcome: std::result::Result<(), String>) {
        self.phase = u8::MAX;
        self.armed = false;
        let s = self.backend.snapshots.borrow().clone();
        let p = self.backend.scripts.borrow().clone();
        let _ = self.backend.stop_script(p.run);
        let _ = self.backend.send(s.epoch, Action::Disconnect);
        if let Ok(mut result) = self.outcome.lock() {
            *result = Some(outcome);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
impl eframe::App for HidCheck {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.phase == u8::MAX {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.armed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let mut abort = false;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Pico keyboard acceptance capture");
            ui.label("Keep this window focused. Do not type during the test.");
            ui.label(
                "Only test letters and Shift are emitted. This window never edits a document.",
            );
            ui.label(format!(
                "Phase {} · {:.1}s",
                self.phase,
                self.clock.elapsed().as_secs_f64()
            ));
            if self.phase == 2 {
                let s = self.backend.snapshots.borrow();
                ui.label(if !s.control_acquired {
                    "Waiting for control acquisition."
                } else if !s.capabilities.as_ref().is_some_and(|c| c.status.usb_ready) {
                    "Waiting for USB keyboard readiness. Connect Pico USB to this computer."
                } else {
                    "Click this capture window if it is not focused."
                });
            }
            abort = ui.button("Abort and disconnect").clicked();
        });
        if abort {
            self.finish(&ctx, Err("Acceptance aborted".into()));
            return;
        }
        match self.step(&ctx) {
            Ok(true) => self.finish(&ctx, Ok(())),
            Err(e) => self.finish(&ctx, Err(e)),
            Ok(false) => {}
        }
        ctx.request_repaint_after(Duration::from_millis(8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(key: egui::Key, pressed: bool, shift: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        }
    }
    #[test]
    fn capture_checks_releases_modifiers_and_bounds_without_retaining_text() {
        let mut c = Capture {
            allow_shift: true,
            ..Default::default()
        };
        c.observe(&[
            key(egui::Key::ShiftLeft, true, true),
            key(egui::Key::B, true, true),
        ])
        .unwrap();
        assert!(!c.released(true));
        c.observe(&[
            key(egui::Key::B, false, true),
            key(egui::Key::ShiftLeft, false, false),
        ])
        .unwrap();
        assert!(c.released(false) && c.shifted_b);
        assert!(c.observe(&[key(egui::Key::Enter, true, false)]).is_err());
        let mut held = Capture::default();
        for _ in 0..128 {
            held.observe(&[
                key(egui::Key::A, true, false),
                key(egui::Key::A, false, false),
            ])
            .unwrap();
        }
        assert!(held.observe(&[key(egui::Key::A, true, false)]).is_err());
        assert!(
            Capture::default()
                .observe(&[key(egui::Key::A, false, false)])
                .is_err()
        );
    }
}

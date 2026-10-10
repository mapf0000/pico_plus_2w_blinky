use crate::backend::{Backend, Mode};
use companion_core::{Action, Connection, Job, Outcome, Snapshot, keyboard, layouts};
use eframe::egui;

const DEMO_TEXT: &str = "Hello from Pico!";

pub struct CompanionApp {
    backend: Backend,
    snapshot: Snapshot,
    mode: Mode,
    selected: Option<String>,
    layout: String,
    text: String,
    delay_ms: u32,
    enqueue_error: Option<&'static str>,
}

impl CompanionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, mode: Mode) -> std::io::Result<Self> {
        let backend = Backend::start(mode, cc.egui_ctx.clone())?;
        Ok(Self {
            backend,
            snapshot: Snapshot::default(),
            mode,
            selected: None,
            layout: "win_en-US".into(),
            text: DEMO_TEXT.into(),
            delay_ms: 0,
            enqueue_error: None,
        })
    }

    fn send(&mut self, action: Action) {
        self.enqueue_error = self.backend.send(self.snapshot.epoch, action).err();
    }

    fn connection(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connection");
        ui.label(self.snapshot.connection.label());
        let transitioning = matches!(
            self.snapshot.connection,
            Connection::Scanning | Connection::Connecting
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.snapshot.pending && self.snapshot.connection != Connection::Connected,
                    egui::Button::new(if self.mode.is_mock() {
                        "Scan mock devices"
                    } else {
                        "Scan Bluetooth"
                    }),
                )
                .clicked()
            {
                self.selected = None;
                self.send(Action::Scan);
            }
            if ui
                .add_enabled(
                    !transitioning
                        && !self.snapshot.pending
                        && self.selected.is_some()
                        && self.snapshot.connection != Connection::Connected,
                    egui::Button::new("Connect"),
                )
                .clicked()
                && let Some(id) = &self.selected
            {
                self.send(Action::Connect(id.clone()));
            }
            if ui
                .add_enabled(
                    transitioning || self.snapshot.connection == Connection::Connected,
                    egui::Button::new("Disconnect"),
                )
                .clicked()
            {
                self.send(Action::Disconnect);
            }
            if transitioning {
                ui.spinner();
            }
        });
        if self.snapshot.devices.is_empty() && !transitioning {
            ui.label("No devices found. Check the device is advertising and scan again.");
        }
        for device in &self.snapshot.devices {
            let selected = self.selected.as_ref() == Some(&device.id);
            let response = ui.add_enabled(
                self.snapshot.connection != Connection::Connected && !transitioning,
                egui::Button::selectable(selected, &device.name),
            );
            if response.clicked() {
                self.selected = Some(device.id.clone());
            }
        }
        if let Some(device) = &self.snapshot.selected {
            ui.label(format!("Selected: {}", device.name));
        }
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.heading("Device status");
        if let Some(caps) = &self.snapshot.capabilities {
            egui::Grid::new("device-status")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Firmware");
                    ui.label(&caps.firmware);
                    ui.end_row();
                    ui.label("Protocol");
                    ui.label(if caps.compatible() {
                        if self.mode.is_mock() {
                            "Compatible (mock)"
                        } else {
                            "BLE control v2"
                        }
                    } else {
                        "Incompatible"
                    });
                    ui.end_row();
                    ui.label("USB keyboard");
                    ui.label(if caps.status.usb_ready {
                        "Ready"
                    } else {
                        "Unavailable"
                    });
                    ui.end_row();
                    ui.label("Host agent");
                    ui.label(if caps.status.host_agent_present {
                        "Present"
                    } else {
                        "Absent"
                    });
                    ui.end_row();
                    if let Some(uptime) = caps.status.uptime_secs {
                        ui.label("Device uptime");
                        ui.label(format!("{uptime} s"));
                        ui.end_row();
                        ui.label("USB enabled");
                        ui.label(if caps.status.usb_enabled { "Yes" } else { "No" });
                        ui.end_row();
                    }
                    ui.label("Control");
                    ui.label(if self.snapshot.control_acquired {
                        "Acquired"
                    } else {
                        if caps.read_only {
                            "Read-only service"
                        } else {
                            "Released"
                        }
                    });
                    ui.end_row();
                    ui.label("Last request");
                    ui.label(
                        self.snapshot
                            .last_rtt
                            .map_or_else(|| "—".into(), |rtt| format!("{} ms", rtt.as_millis())),
                    );
                    ui.end_row();
                });
        } else {
            ui.label("Connect to read capabilities and USB status.");
        }
        ui.horizontal_wrapped(|ui| {
            let can_acquire = self.snapshot.connection == Connection::Connected
                && self.snapshot.compatible()
                && !self.snapshot.control_acquired
                && !self.snapshot.pending;
            if ui
                .add_enabled(can_acquire, egui::Button::new("Pair and acquire control"))
                .clicked()
            {
                self.send(Action::Acquire);
            }
            if ui
                .add_enabled(
                    self.snapshot.control_acquired
                        && !self.snapshot.pending
                        && !self.snapshot.job.active(),
                    egui::Button::new("Release control"),
                )
                .clicked()
            {
                self.send(Action::Release);
            }
            let enabled = self
                .snapshot
                .capabilities
                .as_ref()
                .is_some_and(|c| c.status.usb_enabled);
            if ui
                .add_enabled(
                    self.snapshot.control_acquired
                        && !self.snapshot.pending
                        && !self.snapshot.job.active(),
                    egui::Button::new(if enabled { "Disable USB" } else { "Enable USB" }),
                )
                .clicked()
            {
                self.send(Action::SetUsbEnabled(!enabled));
            }
        });
        if !self.mode.is_mock() {
            ui.small("Compare the pairing code on the Pico and PC. Press X on the Pico to confirm; Y rejects.");
        }
    }

    fn keyboard(&mut self, ui: &mut egui::Ui) {
        ui.heading("Keyboard demonstration");
        ui.label(
            if self.mode.is_mock() { "Execution is simulated." } else { "Types on the computer attached to the Pico USB port. Use an initial delay to focus the target window. Pico Y stops execution." },
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Target input layout");
            let supported = self
                .snapshot
                .capabilities
                .as_ref()
                .map(|caps| &caps.layouts);
            egui::ComboBox::from_id_salt("keyboard-layout")
                .selected_text(&self.layout)
                .show_ui(ui, |ui| {
                    for layout in layouts() {
                        let enabled = supported
                            .is_none_or(|supported| supported.iter().any(|value| value == layout));
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.selectable_value(&mut self.layout, (*layout).into(), *layout);
                        });
                    }
                });
            ui.label("Initial delay");
            ui.add(
                egui::DragValue::new(&mut self.delay_ms)
                    .range(0..=keyboard::MAX_INITIAL_DELAY_MS)
                    .suffix(" ms"),
            );
        });
        ui.label("Text");
        ui.add(
            egui::TextEdit::multiline(&mut self.text)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .char_limit(keyboard::MAX_TEXT_CHARS),
        );
        ui.small(format!(
            "{} / {} characters",
            self.text.chars().count(),
            keyboard::MAX_TEXT_CHARS
        ));
        // Validation stays bounded; typed text never goes to diagnostic output.
        let validation = keyboard::text_effect(self.text.clone(), &self.layout, self.delay_ms);
        let layout_supported = self
            .snapshot
            .capabilities
            .as_ref()
            .is_some_and(|caps| caps.layouts.contains(&self.layout));
        ui.horizontal_wrapped(|ui| {
            if ui.button("Use demo text").clicked() {
                self.text = DEMO_TEXT.into();
            }
            if ui
                .add_enabled(
                    self.snapshot.can_send() && layout_supported && validation.is_ok(),
                    egui::Button::new("Send text"),
                )
                .clicked()
            {
                self.send(Action::SendText {
                    text: self.text.clone(),
                    layout: self.layout.clone(),
                    delay_ms: self.delay_ms,
                });
            }
            if ui
                .add_enabled(
                    self.snapshot.job.active() && self.snapshot.job != Job::Cancelling,
                    egui::Button::new("Cancel effect"),
                )
                .clicked()
            {
                self.send(Action::Cancel);
            }
            if self.snapshot.job.active() {
                ui.spinner();
            }
        });
        if let Err(error) = validation {
            ui.colored_label(ui.visuals().error_fg_color, error.message());
        }
        ui.label(match self.snapshot.job {
            Job::Idle => "No effect submitted",
            Job::Sending => "Submitting effect…",
            Job::Running => "Effect running",
            Job::Cancelling => "Cancelling current effect…",
            Job::Finished(Outcome::Completed) => "Effect completed",
            Job::Finished(Outcome::Cancelled) => "Effect cancelled",
            Job::Finished(Outcome::Rejected) => "Effect rejected",
            Job::Finished(Outcome::UsbUnavailable) => "USB unavailable",
            Job::Finished(Outcome::Disconnected) => {
                "Effect ended on disconnect; it will not be replayed"
            }
        });
    }
}

impl eframe::App for CompanionApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.backend.snapshots.has_changed().unwrap_or(false) {
            self.snapshot = self.backend.snapshots.borrow_and_update().clone();
            if self.selected.is_none() {
                self.selected = self
                    .snapshot
                    .devices
                    .first()
                    .map(|device| device.id.clone());
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Pico Companion");
                match self.mode {
                    Mode::Mock(scenario) => {
                        ui.label(format!(
                            "Mock mode · {} scenario · no Bluetooth or USB access",
                            scenario.name()
                        ));
                    }
                    Mode::Ble => {
                        ui.label("Bluetooth · Pico control");
                    }
                }
                if let Some(error) = self.enqueue_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(error) = self.snapshot.last_error {
                    ui.colored_label(ui.visuals().error_fg_color, error.message());
                }
                ui.separator();
                self.connection(ui);
                ui.separator();
                self.status(ui);
                ui.separator();
                self.keyboard(ui);
                ui.separator();
                ui.heading("Diagnostics");
                ui.small(format!(
                    "Last {} entries; {} earlier entries discarded",
                    self.snapshot.diagnostics.len(),
                    self.snapshot.discarded_diagnostics
                ));
                egui::ScrollArea::vertical()
                    .id_salt("diagnostics")
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for message in &self.snapshot.diagnostics {
                            ui.monospace(*message);
                        }
                    });
            });
        });
    }
}

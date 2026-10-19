use crate::backend::{Backend, Mode};
use companion_core::{
    Action, Connection, IdentityUpdate, Job, Outcome, Snapshot, UsbIdentity, keyboard, layouts,
};
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
    diagnostic_filter: String,
    errors_only: bool,
    autoscroll: bool,
    manufacturer: String,
    product: String,
    identity_epoch: u64,
    identity_revision: u64,
    filesystem_path: String,
    filesystem_hidden: bool,
    download_path: String,
    download_destination: String,
    script_source: String,
    script: crate::python::ScriptSnapshot,
}

impl CompanionApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        mode: Mode,
        profile: Option<ble_session::provisioning::Profile>,
    ) -> std::io::Result<Self> {
        let backend = Backend::start_with_profile(mode, profile, cc.egui_ctx.clone())?;
        Ok(Self {
            backend,
            snapshot: Snapshot::default(),
            mode,
            selected: None,
            layout: "win_en-US".into(),
            text: DEMO_TEXT.into(),
            delay_ms: 0,
            enqueue_error: None,
            diagnostic_filter: String::new(),
            errors_only: false,
            autoscroll: true,
            manufacturer: String::new(),
            product: String::new(),
            identity_epoch: 0,
            identity_revision: 0,
            filesystem_path: String::new(),
            filesystem_hidden: false,
            download_path: String::new(),
            download_destination: String::new(),
            script_source: crate::python::DEMO.into(),
            script: crate::python::ScriptSnapshot::default(),
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
                            "BLE control v3"
                        }
                    } else {
                        "Incompatible"
                    });
                    ui.end_row();
                    ui.label("Companion layouts");
                    ui.label(caps.layouts.join(", "));
                    ui.end_row();
                    if let Some(overview) = &caps.overview {
                        let overview = overview.value();
                        for (label, value) in [
                            ("Agent version", overview.agent_version),
                            ("Host OS", overview.host_os),
                            ("Hostname", overview.hostname),
                        ] {
                            ui.label(label);
                            ui.label(if value.is_empty() { "Unknown" } else { value });
                            ui.end_row();
                        }
                        ui.label("Heartbeat age at overview");
                        ui.label(overview.agent_age_ms.map_or_else(
                            || "Never seen".into(),
                            |age| format!("{:.1} s", age as f64 / 1000.0),
                        ));
                        ui.end_row();
                        ui.label("CDC installer");
                        ui.label(if overview.bootstrap_available {
                            overview.bootstrap.label()
                        } else {
                            "Not packaged"
                        });
                        ui.end_row();
                        ui.label("Device effect limits");
                        ui.label(format!(
                            "{} bytes · {} ms per delay · {} s total delays",
                            overview.max_bytecode,
                            overview.max_delay_ms,
                            overview.max_total_delay_ms / 1000
                        ));
                        ui.end_row();
                        ui.label("USB agent protocols");
                        ui.label(format!(
                            "Filesystem v{} · transfer v{} · {} byte payloads",
                            overview.filesystem_version,
                            overview.transfer_version,
                            overview.max_usb_payload
                        ));
                        ui.end_row();
                    }
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
                .add_enabled(
                    can_acquire,
                    egui::Button::new("Authenticate and acquire control"),
                )
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
        });
        if let Some(caps) = &self.snapshot.capabilities {
            if !caps.overview_supported {
                ui.small("This firmware does not advertise a detailed overview.");
            } else if caps.overview.is_none() {
                ui.small("Acquire control to read the protected device overview.");
            } else {
                ui.small("Overview refreshes every 10 seconds while control is acquired. USB status refreshes every second.");
            }
        }
        if !self.mode.is_mock() {
            ui.small("Uses the provisioned device key. No Pico button press is required.");
        }
    }

    fn usb_controls(&mut self, ui: &mut egui::Ui) {
        let status = self.snapshot.capabilities.as_ref().map(|caps| caps.status);
        let can_control =
            self.snapshot.control_acquired && !self.snapshot.pending && !self.snapshot.job.active();
        ui.horizontal_wrapped(|ui| {
            ui.label(match status {
                Some(status) if status.usb_enabled => "USB enabled",
                Some(_) => "USB disabled",
                None => "USB status unavailable",
            });
            if ui
                .add_enabled(
                    can_control && status.is_some_and(|status| !status.usb_enabled),
                    egui::Button::new("Enable USB"),
                )
                .clicked()
            {
                self.send(Action::SetUsbEnabled(true));
            }
            if ui
                .add_enabled(
                    can_control
                        && status.is_some_and(|status| status.usb_enabled || status.usb_ready),
                    egui::Button::new("Disable USB"),
                )
                .clicked()
            {
                self.send(Action::SetUsbEnabled(false));
            }
        });
        if !self.snapshot.control_acquired {
            ui.small("Connect and acquire control to enable or disable USB.");
        } else if self.snapshot.job.active() {
            ui.small("Finish or cancel the keyboard effect before changing USB.");
        } else if self.snapshot.pending {
            ui.small("Waiting for the current command to finish.");
        }
    }

    fn usb_identity(&mut self, ui: &mut egui::Ui) {
        ui.heading("USB identity");
        self.usb_controls(ui);
        let supported = self
            .snapshot
            .capabilities
            .as_ref()
            .is_some_and(|caps| caps.identity_supported);
        if !supported {
            ui.label("Connect to firmware that supports USB identity editing.");
            return;
        }
        let loaded = self
            .snapshot
            .capabilities
            .as_ref()
            .and_then(|caps| caps.identity.as_ref());
        let editable = self.snapshot.can_read_identity() && loaded.is_some();
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("usb-identity")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Manufacturer");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.manufacturer)
                            .char_limit(32)
                            .desired_width(300.0),
                    );
                    ui.end_row();
                    ui.label("Product");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.product)
                            .char_limit(48)
                            .desired_width(300.0),
                    );
                    ui.end_row();
                });
        });
        ui.small("Printable ASCII: manufacturer up to 32 bytes, product up to 48. Empty strings are allowed.");
        ui.label("Disable USB, save, then enable USB to apply the new names.");
        let value = UsbIdentity::new(&self.manufacturer, &self.product);
        let dirty = value
            .as_ref()
            .ok()
            .is_some_and(|value| Some(value) != loaded);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.snapshot.can_read_identity(),
                    egui::Button::new("Reload identity"),
                )
                .clicked()
            {
                self.send(Action::ReadIdentity);
            }
            if ui
                .add_enabled(
                    self.snapshot.can_save_identity() && value.is_ok() && dirty,
                    egui::Button::new("Save identity"),
                )
                .clicked()
                && let Ok(value) = value
            {
                self.send(Action::SaveIdentity(value));
            }
        });
        if let Err(error) = UsbIdentity::new(&self.manufacturer, &self.product) {
            ui.colored_label(ui.visuals().error_fg_color, error.message());
        }
        match self.snapshot.identity_update {
            IdentityUpdate::Unknown => {
                ui.small("Acquire control to read the saved identity.");
            }
            IdentityUpdate::Loaded => {
                ui.small("Identity read from flash.");
            }
            IdentityUpdate::Saved => {
                ui.small("Saved and verified in flash. Applies on the next USB start.");
            }
            IdentityUpdate::Failed(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error.message());
            }
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
        ui.small(format!(
            "Effect limit: {} bytes of KBD1",
            script_protocol::MAX_BYTECODE
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
                    self.snapshot.can_send()
                        && !self.script.state.active()
                        && layout_supported
                        && validation.is_ok(),
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
                    self.snapshot.job.active()
                        && !self.script.state.active()
                        && self.snapshot.job != Job::Cancelling,
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
            Job::Queued => "Waiting for status read…",
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

    fn filesystem(&mut self, ui: &mut egui::Ui) {
        use companion_core::filesystem::{breadcrumbs, join, parent, root};
        ui.heading("Host filesystem");
        let browser = self.snapshot.filesystem.clone();
        let available = self.snapshot.can_browse();
        if !available && !browser.loading {
            ui.weak(
                if self
                    .snapshot
                    .capabilities
                    .as_ref()
                    .is_some_and(|c| !c.filesystem_supported)
                {
                    "This firmware does not support Bluetooth filesystem browsing."
                } else if !self.snapshot.control_acquired {
                    "Connect and acquire control to browse the USB computer."
                } else {
                    "Enable USB and start the host agent on the attached computer."
                },
            );
        }
        let mut browse = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(available, egui::Button::new("Home"))
                .clicked()
            {
                browse = Some((String::new(), 0));
            }
            if ui
                .add_enabled(available, egui::Button::new("Root"))
                .clicked()
            {
                browse = Some((root(&browser.directory), 0));
            }
            if ui
                .add_enabled(
                    available && !browser.directory.is_empty(),
                    egui::Button::new("Up"),
                )
                .clicked()
            {
                browse = Some((parent(&browser.directory), 0));
            }
            if ui
                .add_enabled(available, egui::Button::new("Refresh directory"))
                .clicked()
            {
                browse = Some((browser.directory.clone(), 0));
            }
            if ui
                .add_enabled(browser.loading, egui::Button::new("Cancel listing"))
                .clicked()
            {
                self.send(Action::CancelDirectory);
            }
            if browser.loading {
                ui.spinner();
                ui.label("Listing…");
            }
        });
        ui.horizontal(|ui| {
            ui.label("Path");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.filesystem_path)
                    .char_limit(ble_protocol::filesystem::PATH_MAX)
                    .hint_text("Empty path opens the host's home"),
            );
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui
                .add_enabled(available, egui::Button::new("Open"))
                .clicked()
                || enter)
                && available
            {
                browse = Some((self.filesystem_path.clone(), 0));
            }
        });
        if ui
            .add_enabled(
                available,
                egui::Checkbox::new(&mut self.filesystem_hidden, "Show hidden files"),
            )
            .changed()
        {
            browse = Some((browser.directory.clone(), 0));
        }
        ui.horizontal_wrapped(|ui| {
            for (label, path) in breadcrumbs(&browser.directory) {
                if ui
                    .add_enabled(available, egui::Button::new(label))
                    .clicked()
                {
                    browse = Some((path, 0));
                }
            }
        });
        if let Some(error) = &browser.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if !browser.directory.is_empty() {
            ui.label(&browser.directory);
            if browser.entries.is_empty() && !browser.loading && browser.error.is_none() {
                ui.weak("This directory is empty.");
            }
        }
        egui::ScrollArea::vertical()
            .id_salt("filesystem-entries")
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("filesystem-table")
                    .striped(true)
                    .num_columns(4)
                    .show(ui, |ui| {
                        ui.strong("Name");
                        ui.strong("Type");
                        ui.strong("Bytes");
                        ui.strong("Modified (Unix seconds)");
                        ui.end_row();
                        for entry in &browser.entries {
                            if let Some(path) = join(&browser.directory, &entry.name) {
                                let response = ui.selectable_label(
                                    browser.selected.as_ref() == Some(&path),
                                    &entry.name,
                                );
                                if response.clicked() {
                                    self.send(Action::SelectPath(path.clone()));
                                }
                                if response.double_clicked() && entry.directory() && available {
                                    browse = Some((path, 0));
                                }
                            } else {
                                ui.add_enabled(false, egui::Button::selectable(false, &entry.name))
                                    .on_disabled_hover_text("Full path exceeds the 512-byte limit");
                            }
                            ui.label(entry.kind_label());
                            ui.label(if entry.directory() {
                                "—".into()
                            } else {
                                entry.size.to_string()
                            });
                            ui.label(if entry.modified == u64::MAX {
                                "Unknown".into()
                            } else {
                                entry.modified.to_string()
                            });
                            ui.end_row();
                        }
                    });
            });
        if let Some(selected) = &browser.selected {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Selected: {selected}"));
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(selected.clone());
                }
            });
        }
        if ui
            .add_enabled(available && browser.more, egui::Button::new("Load more"))
            .clicked()
        {
            browse = Some((browser.directory.clone(), browser.cursor));
        }
        ui.weak(format!(
            "{} of at most 512 entries. Double-click a folder to open it.",
            browser.entries.len()
        ));
        if let Some(button) = self.snapshot.buttons.back() {
            ui.label(format!(
                "Last Pico button released: {}",
                ["A", "B", "X", "Y"][*button as usize]
            ));
        }
        if let Some((path, cursor)) = browse {
            self.send(Action::Browse {
                path,
                cursor,
                hidden: self.filesystem_hidden,
            });
        }
    }

    fn downloads(&mut self, ui: &mut egui::Ui) {
        use companion_core::download::{Command, Session, State};
        ui.heading("Encrypted file downloads");
        let supported = self
            .snapshot
            .capabilities
            .as_ref()
            .is_some_and(|c| c.downloads_supported);
        let available = supported
            && !self.snapshot.job.active()
            && self.snapshot.control_acquired
            && self
                .snapshot
                .capabilities
                .as_ref()
                .is_some_and(|c| c.status.host_agent_present);
        if self.snapshot.connection != Connection::Connected {
            ui.label("Connect to the Pico to use encrypted downloads.");
        } else if !supported {
            ui.label("This firmware does not support native downloads.");
        } else if !self.snapshot.control_acquired {
            ui.label("Acquire control to use encrypted downloads.");
        } else if !self
            .snapshot
            .capabilities
            .as_ref()
            .is_some_and(|c| c.status.host_agent_present)
        {
            ui.label("Start the host agent on the computer connected to Pico USB.");
        } else if self.snapshot.job.active() {
            ui.label("Stop the current keyboard effect before issuing download controls.");
        }
        ui.label(format!(
            "Host session: {:?}",
            self.snapshot.downloads.session
        ));
        ui.label("Choose a new absolute local filename. Existing files are never overwritten. Maximum file size: 1 MiB.");
        let mut command = None;
        if ui
            .add_enabled(
                available
                    && !self.snapshot.downloads.active()
                    && self.snapshot.downloads.session != Session::Negotiating,
                egui::Button::new("Connect encrypted host session"),
            )
            .clicked()
        {
            command = Some(Command::ConnectSession);
        }
        ui.horizontal(|ui| {
            ui.label("Host source path");
            ui.add(
                egui::TextEdit::singleline(&mut self.download_path)
                    .char_limit(512)
                    .desired_width(f32::INFINITY),
            );
        });
        if ui.button("Use selected filesystem path").clicked()
            && let Some(path) = &self.snapshot.filesystem.selected
        {
            self.download_path = path.clone();
        }
        ui.horizontal(|ui| {
            ui.label("Save to local filename");
            ui.add(
                egui::TextEdit::singleline(&mut self.download_destination)
                    .char_limit(4096)
                    .desired_width(f32::INFINITY),
            );
        });
        let destination = std::path::PathBuf::from(&self.download_destination);
        let ready = available && self.snapshot.downloads.session == Session::Ready;
        let queue = Command::Queue {
            path: self.download_path.clone(),
            destination: destination.clone(),
        };
        let default = Command::QueueDefault { destination };
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(ready && queue.valid(), egui::Button::new("Queue file"))
                .clicked()
            {
                command = Some(queue);
            }
            if ui
                .add_enabled(
                    ready && Command::SetDefault(self.download_path.clone()).valid(),
                    egui::Button::new("Set host default path"),
                )
                .clicked()
            {
                command = Some(Command::SetDefault(self.download_path.clone()));
            }
            if ui
                .add_enabled(
                    ready && default.valid(),
                    egui::Button::new("Queue host default"),
                )
                .clicked()
            {
                command = Some(default);
            }
            if ui
                .add_enabled(
                    available && self.snapshot.downloads.active(),
                    egui::Button::new("Cancel downloads"),
                )
                .clicked()
            {
                command = Some(Command::Cancel);
            }
        });
        if let Some(command) = command {
            self.send(Action::Download(command));
        }
        if let Some(error) = &self.snapshot.downloads.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        for item in &self.snapshot.downloads.items {
            ui.group(|ui| {
                ui.label(format!(
                    "{} — {:?}",
                    if item.name.is_empty() {
                        "Pending file"
                    } else {
                        &item.name
                    },
                    item.state
                ));
                ui.label(item.destination.display().to_string());
                if matches!(
                    item.state,
                    State::Receiving | State::Verifying | State::Saved
                ) {
                    let fraction = if item.total == 0 {
                        1.0
                    } else {
                        item.received as f32 / item.total as f32
                    };
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    let remaining = item.total.saturating_sub(item.received) as f64
                        / item.bytes_per_second.max(1.0);
                    ui.small(format!(
                        "{} / {} bytes · {:.0} bytes/s · {:.0}s remaining · {} retries",
                        item.received, item.total, item.bytes_per_second, remaining, item.retries
                    ));
                }
                if let Some(error) = &item.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        }
    }

    fn scripting(&mut self, ui: &mut egui::Ui) {
        ui.heading("Python scripting");
        ui.label("Python runs on this computer. Yield effects to control the USB computer; edits apply on the next Start.");
        ui.add(
            egui::TextEdit::multiline(&mut self.script_source)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .desired_rows(10)
                .desired_width(f32::INFINITY)
                .char_limit(python_runtime::MAX_SOURCE_BYTES),
        );
        let invalid = python_runtime::source_limit_error(&self.script_source);
        let available = invalid.is_none()
            && (!self.snapshot.job.active() || self.snapshot.job_owner == Some(self.script.run));
        let shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        let mut start = shortcut && available;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    available,
                    egui::Button::new(if self.script.state.active() {
                        "Restart script"
                    } else {
                        "Start script"
                    }),
                )
                .clicked()
            {
                start = true;
            }
            if ui
                .add_enabled(self.script.state.active(), egui::Button::new("Stop script"))
                .clicked()
            {
                self.enqueue_error = self.backend.stop_script(self.script.run).err();
            }
            if ui.button("Use Python demo").clicked() {
                self.script_source = crate::python::DEMO.into();
            }
            ui.label("Ctrl/Cmd+Enter to start");
        });
        if start {
            self.enqueue_error = self
                .backend
                .start_script(self.snapshot.epoch, self.script_source.clone())
                .err();
        }
        ui.label(self.script.state.label());
        ui.small(format!(
            "{} / 32,768 source bytes · {} completed keyboard effects",
            self.script_source.len(),
            self.script.completed_effects
        ));
        if let Some(layout) = &self.script.layout {
            ui.label(format!("Script input layout: {layout}"));
        }
        if self.script.heap_peak != 0 {
            ui.small(format!(
                "Peak Python Rust heap: {:.1} MiB / 128 MiB",
                self.script.heap_peak as f64 / 1_048_576.0
            ));
        }
        if let Some(error) = invalid {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(error) = &self.script.error {
            egui::ScrollArea::vertical()
                .id_salt("python-error")
                .max_height(150.0)
                .show(ui, |ui| {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        egui::RichText::new(error).monospace(),
                    );
                });
        }
        egui::CollapsingHeader::new("Python API and examples").show(ui, |ui| {
            ui.monospace("layout(\"win_en-US\")\ndef main():\n    yield require_usb()\n    yield sleep(5000)\n    yield text(\"Hello\", 20)");
            ui.label("Effects: tap(key), modtap(chord), text(value, delay_ms=10), sleep(ms), wait_event(*kinds, timeout_ms=None), require_usb(), require_host_agent().");
            ui.label("Events: button, connection, usb, host_agent. Button values are A/B/X/Y with edge released; a timed wait returns a timer event.");
            ui.label("Catch PicoError, DeviceDisconnected, UsbUnavailable, HostAgentUnavailable, EffectRejected or EffectCancelled. Stop terminates Python immediately without running cleanup.");
            ui.label("Imports, files, network, host commands and credentials are unavailable. One generator/effect; startup 30 seconds, later steps 500 ms. Select the USB computer's actual input layout.");
        });
    }

    fn diagnostics(&mut self, ui: &mut egui::Ui) {
        ui.heading("Diagnostics");
        ui.horizontal_wrapped(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.diagnostic_filter)
                    .char_limit(128)
                    .desired_width(180.0),
            );
            ui.checkbox(&mut self.errors_only, "Errors only");
            ui.checkbox(&mut self.autoscroll, "Autoscroll");
            if ui.button("Clear").clicked() {
                self.send(Action::ClearDiagnostics);
            }
        });
        let filter = self.diagnostic_filter.to_lowercase();
        let entries: Vec<_> = self
            .snapshot
            .diagnostics
            .iter()
            .filter(|entry| {
                (!self.errors_only || entry.error.is_some())
                    && (entry.message.to_lowercase().contains(&filter)
                        || entry.context.label().to_lowercase().contains(&filter))
            })
            .collect();
        ui.small(format!(
            "Showing {} of {} entries; {} earlier entries discarded",
            entries.len(),
            self.snapshot.diagnostics.len(),
            self.snapshot.discarded_diagnostics
        ));
        egui::ScrollArea::vertical()
            .id_salt("diagnostics")
            .max_height(180.0)
            .stick_to_bottom(self.autoscroll)
            .show(ui, |ui| {
                for entry in entries {
                    let label = format!(
                        "{:>7.2}s · session {} · {} · {}",
                        entry.at.as_secs_f64(),
                        entry.epoch,
                        entry.context.label(),
                        entry.message
                    );
                    if entry.error.is_some() {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            egui::RichText::new(label).monospace(),
                        );
                    } else {
                        ui.monospace(label);
                    }
                }
            });
    }
}

impl eframe::App for CompanionApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.backend.snapshots.has_changed().unwrap_or(false) {
            self.snapshot = self.backend.snapshots.borrow_and_update().clone();
            if self.identity_epoch != self.snapshot.epoch {
                self.identity_epoch = self.snapshot.epoch;
                self.manufacturer.clear();
                self.product.clear();
            }
            if self.snapshot.capabilities.is_none() {
                self.manufacturer.clear();
                self.product.clear();
            }
            if self.identity_revision != self.snapshot.identity_revision {
                self.identity_revision = self.snapshot.identity_revision;
                // A rejected save returns the current identity but retains the draft.
                if matches!(
                    self.snapshot.identity_update,
                    IdentityUpdate::Loaded | IdentityUpdate::Saved
                ) && let Some(value) = self
                    .snapshot
                    .capabilities
                    .as_ref()
                    .and_then(|caps| caps.identity.as_ref())
                {
                    self.manufacturer = value.value().manufacturer.into();
                    self.product = value.value().product.into();
                }
            }
            if self.selected.is_none() {
                self.selected = self
                    .snapshot
                    .devices
                    .first()
                    .map(|device| device.id.clone());
            }
        }
        if self.backend.scripts.has_changed().unwrap_or(false) {
            self.script = self.backend.scripts.borrow_and_update().clone();
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
                self.usb_identity(ui);
                ui.separator();
                self.keyboard(ui);
                ui.separator();
                self.scripting(ui);
                ui.separator();
                self.filesystem(ui);
                ui.separator();
                self.downloads(ui);
                ui.separator();
                self.diagnostics(ui);
            });
        });
    }
}

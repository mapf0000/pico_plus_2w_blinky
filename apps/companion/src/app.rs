use crate::backend::{Backend, Mode};
use crate::ui::state::{self, Workspace};
use crate::ui::{components as c, theme};
use companion_core::{
    Action, Connection, IdentityUpdate, Job, Snapshot, UsbIdentity, keyboard, layouts,
};
use eframe::egui;

#[path = "ui/devices.rs"]
mod devices;
#[path = "ui/scripts.rs"]
mod scripts;
#[path = "ui/shell.rs"]
mod shell;

const DEMO_TEXT: &str = "Hello from Pico!";
struct ProfileInstall {
    result: tokio::sync::oneshot::Receiver<Result<(), &'static str>>,
    id: String,
}

pub struct CompanionApp {
    backend: Backend,
    workspace: Workspace,
    activity_open: bool,
    validation: state::TextValidation,
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
    desktop: crate::desktop::Desktop,
    catalog: crate::profiles::Catalog,
    active_profile: Option<String>,
    profile_install: Option<ProfileInstall>,
    desktop_error: Option<&'static str>,
    destination_error: Option<&'static str>,
}

impl CompanionApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        mode: Mode,
        profile: Option<ble_session::provisioning::Profile>,
    ) -> std::io::Result<Self> {
        Self::from_context(&cc.egui_ctx, mode, profile)
    }

    fn from_context(
        ctx: &egui::Context,
        mode: Mode,
        profile: Option<ble_session::provisioning::Profile>,
    ) -> std::io::Result<Self> {
        crate::ui::theme::apply(ctx);
        let active_profile = profile.as_ref().map(crate::profiles::id);
        let desktop =
            crate::desktop::Desktop::new(ctx.clone(), !mode.is_mock(), profile.is_none())?;
        let backend = Backend::start_with_profile(mode, profile, ctx.clone())?;
        Ok(Self {
            backend,
            workspace: Workspace::Overview,
            activity_open: false,
            validation: state::TextValidation::default(),
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
            desktop,
            catalog: crate::profiles::Catalog::default(),
            active_profile,
            profile_install: None,
            desktop_error: None,
            destination_error: None,
        })
    }

    fn desktop_busy(&self) -> bool {
        self.desktop.busy || self.profile_install.is_some()
    }

    fn poll_desktop(&mut self) {
        if let Some(pending) = &mut self.profile_install {
            match pending.result.try_recv() {
                Ok(Ok(())) => {
                    self.active_profile = Some(pending.id.clone());
                    self.selected = None;
                    self.desktop_error = None;
                    self.profile_install = None;
                }
                Ok(Err(error)) => {
                    self.desktop_error = Some(error);
                    self.profile_install = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.desktop_error = Some("Profile update stopped; restart the app");
                    self.profile_install = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(reply) = self.desktop.poll() {
            match reply {
                crate::desktop::Reply::Profiles { catalog, profile } => {
                    self.desktop_error = None;
                    self.catalog = catalog;
                    if let Some(profile) = profile {
                        let id = crate::profiles::id(&profile);
                        let epoch = self.backend.snapshots.borrow().epoch;
                        match self.backend.set_profile(epoch, Some(profile)) {
                            Ok(result) => {
                                self.profile_install = Some(ProfileInstall { result, id })
                            }
                            Err(error) => self.desktop_error = Some(error),
                        }
                    }
                }
                crate::desktop::Reply::Destination {
                    path,
                    context,
                    draft,
                } => {
                    self.destination_error = crate::desktop::apply_destination(
                        context,
                        &draft,
                        &path,
                        crate::desktop::DestinationContext::from_snapshot(
                            &self.backend.snapshots.borrow(),
                        ),
                        &mut self.download_destination,
                    )
                    .err();
                }
                crate::desktop::Reply::DestinationError(error) => {
                    self.destination_error = Some(error)
                }
                crate::desktop::Reply::Error(error) => self.desktop_error = Some(error),
                crate::desktop::Reply::Cancelled => {}
            }
        }
    }

    fn profiles(&mut self, ui: &mut egui::Ui) {
        if self.mode.is_mock() {
            return;
        }
        let label = self.active_profile.as_ref().map_or_else(
            || "No profile loaded · public status only".into(),
            |id| {
                let name = self
                    .catalog
                    .profiles
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or("Launch profile", |p| p.name.as_str());
                format!("{name} · {}", &id[..8])
            },
        );
        let supported = cfg!(target_os = "macos");
        let change = supported
            && !self.desktop_busy()
            && !self.script.state.active()
            && !self.snapshot.job.active()
            && !matches!(
                self.snapshot.connection,
                Connection::Connected | Connection::Connecting
            );
        egui::CollapsingHeader::new(format!("Provisioning profile · {label}"))
            .id_salt("profiles").default_open(self.active_profile.is_none()).show(ui, |ui| {
                ui.label("Import once to keep the device key in macOS Keychain. The last selection loads on next launch.");
                let mut selection = self.active_profile.clone();
                ui.add_enabled_ui(change, |ui| {
                    egui::ComboBox::from_id_salt("profile-selection").selected_text(&label).show_ui(ui, |ui| {
                        for profile in &self.catalog.profiles {
                            ui.selectable_value(&mut selection, Some(profile.id.clone()),
                                format!("{} · {}", profile.name, &profile.id[..8]));
                        }
                    });
                });
                if selection != self.active_profile && let Some(id) = selection {
                    self.desktop_error = self.desktop.select(id).err();
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(change, egui::Button::new("Import profile…")).clicked() {
                        self.desktop_error = self.desktop.import().err();
                    }
                    if ui.add_enabled(change, egui::Button::new("Load last saved profile")).clicked() {
                        self.desktop_error = self.desktop.load().err();
                    }
                });
                if !supported { ui.label(crate::profiles::Error::Unsupported.message()); }
                else if !change && !self.desktop_busy() {
                    ui.small("Disconnect and stop Python before changing the active profile.");
                }
            });
        if self.desktop_busy() {
            ui.label("Waiting for local dialog or credential storage…");
        }
    }

    fn send(&mut self, action: Action) {
        self.enqueue_error = self.backend.send(self.snapshot.epoch, action).err();
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.heading("Overview");
        c::description(
            ui,
            "Connection, USB and the computer attached to your Pico.",
        );
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
        if self.mode.is_mock() {
            ui.small("This overview is simulated; no Bluetooth or USB access.");
        }
    }

    fn has_profile(&self) -> bool {
        self.mode.is_mock() || self.active_profile.is_some()
    }

    fn readiness_action(&mut self, ui: &mut egui::Ui, action: state::ReadinessAction) {
        let enabled = match action {
            state::ReadinessAction::Devices => true,
            state::ReadinessAction::Acquire => {
                state::acquire_blocker(&self.snapshot, self.has_profile()).is_none()
            }
            state::ReadinessAction::EnableUsb => {
                !self.snapshot.pending && !self.snapshot.job.active()
            }
        };
        if c::primary(ui, enabled, action.label()).clicked() {
            match action {
                state::ReadinessAction::Devices => self.workspace = Workspace::Devices,
                state::ReadinessAction::Acquire => self.send(Action::Acquire),
                state::ReadinessAction::EnableUsb => self.send(Action::SetUsbEnabled(true)),
            }
        }
        if !enabled {
            ui.small("Waiting for the current command to finish.");
        }
    }

    fn readiness(&mut self, ui: &mut egui::Ui) {
        let readiness = state::usb_readiness(&self.snapshot, self.has_profile());
        theme::card(ui).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            c::badge(
                ui,
                if readiness.ready {
                    "Ready"
                } else {
                    "Next step"
                },
                !readiness.ready,
            );
            ui.label(readiness.message);
            if let Some(action) = readiness.action {
                self.readiness_action(ui, action);
            }
        });
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
            if c::quiet(
                ui,
                can_control && status.is_some_and(|status| !status.usb_enabled),
                "Enable USB",
            )
            .clicked()
            {
                self.send(Action::SetUsbEnabled(true));
            }
            if c::quiet(
                ui,
                can_control && status.is_some_and(|status| status.usb_enabled || status.usb_ready),
                "Disable USB",
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
        c::section(ui, "USB identity");
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
            ui.label("Manufacturer");
            ui.add(
                egui::TextEdit::singleline(&mut self.manufacturer)
                    .char_limit(32)
                    .desired_width(f32::INFINITY),
            );
            ui.label("Product");
            ui.add(
                egui::TextEdit::singleline(&mut self.product)
                    .char_limit(48)
                    .desired_width(f32::INFINITY),
            );
        });
        ui.small("Printable ASCII: manufacturer up to 32 bytes, product up to 48. Empty strings are allowed.");
        ui.label("Disable USB, save, then enable USB to apply the new names.");
        let value = UsbIdentity::new(&self.manufacturer, &self.product);
        let dirty = value
            .as_ref()
            .ok()
            .is_some_and(|value| Some(value) != loaded);
        ui.horizontal_wrapped(|ui| {
            if c::quiet(ui, self.snapshot.can_read_identity(), "Reload identity").clicked() {
                self.send(Action::ReadIdentity);
            }
            if c::primary(
                ui,
                self.snapshot.can_save_identity() && value.is_ok() && dirty,
                "Save identity",
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
        ui.heading("Keyboard");
        c::description(
            ui,
            if self.mode.is_mock() {
                "Simulated typing · no keyboard output leaves this app."
            } else {
                "Types on the USB computer. Add a delay to focus its target window. Pico Y stops execution."
            },
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Target input layout");
            let supported = self
                .snapshot
                .capabilities
                .as_ref()
                .map(|caps| &caps.layouts);
            egui::ComboBox::from_id_salt("keyboard-layout")
                .selected_text(state::layout_name(&self.layout))
                .show_ui(ui, |ui| {
                    for layout in layouts() {
                        let enabled = supported
                            .is_none_or(|supported| supported.iter().any(|value| value == layout));
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.selectable_value(
                                &mut self.layout,
                                (*layout).into(),
                                state::layout_name(layout),
                            );
                        });
                    }
                });
            ui.label("Delay before typing");
            ui.add(
                egui::DragValue::new(&mut self.delay_ms)
                    .range(0..=keyboard::MAX_INITIAL_DELAY_MS)
                    .suffix(" ms"),
            );
        });
        ui.label("Text");
        ui.add(
            egui::TextEdit::multiline(&mut self.text)
                .desired_rows(8)
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
        self.validation
            .update(&self.text, &self.layout, self.delay_ms);
        let validation = self.validation.summary;
        let blocker =
            state::typing_blocker(&self.snapshot, self.script.state.active(), &self.layout);
        ui.horizontal_wrapped(|ui| {
            if c::quiet(ui, true, "Use demo text").clicked() {
                self.text = DEMO_TEXT.into();
            }
            if c::primary(
                ui,
                self.snapshot.can_send()
                    && blocker.is_none()
                    && validation.is_some_and(|v| v.is_ok()),
                "Send text",
            )
            .clicked()
            {
                self.send(Action::SendText {
                    text: self.text.clone(),
                    layout: self.layout.clone(),
                    delay_ms: self.delay_ms,
                });
            }
            if c::stop(
                ui,
                self.snapshot.job.active()
                    && !self.script.state.active()
                    && self.snapshot.job != Job::Cancelling,
                "Cancel effect",
            )
            .clicked()
            {
                self.send(Action::Cancel);
            }
            if self.snapshot.job.active() {
                ui.spinner();
            }
        });
        if let Some(Err(error)) = validation {
            ui.colored_label(ui.visuals().error_fg_color, error.message());
        }
        if let Some(reason) = blocker {
            ui.label(reason);
        }
        if let Some(Ok((bytes, duration))) = validation {
            ui.small(format!(
                "{bytes} encoded bytes · approximately {:.1} s including delay",
                duration as f64 / 1000.0
            ));
        }
        ui.label(state::job_label(self.snapshot.job));
    }

    fn filesystem(&mut self, ui: &mut egui::Ui) {
        use companion_core::filesystem::{breadcrumbs, join, parent, root};
        ui.heading("Files");
        c::description(ui, "Browse the computer connected to Pico USB.");
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
            if c::quiet(ui, available, "Home").clicked() {
                browse = Some((String::new(), 0));
            }
            if c::quiet(ui, available, "Root").clicked() {
                browse = Some((root(&browser.directory), 0));
            }
            if c::quiet(ui, available && !browser.directory.is_empty(), "Up").clicked() {
                browse = Some((parent(&browser.directory), 0));
            }
            if c::quiet(ui, available, "Refresh directory").clicked() {
                browse = Some((browser.directory.clone(), 0));
            }
            if c::quiet(ui, browser.loading, "Cancel listing").clicked() {
                self.send(Action::CancelDirectory);
            }
            if browser.loading {
                ui.spinner();
                ui.label("Listing…");
            }
        });
        ui.label("Path");
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.filesystem_path)
                    .desired_width((ui.available_width() - 80.0).max(80.0))
                    .char_limit(ble_protocol::filesystem::PATH_MAX)
                    .hint_text("Empty path opens the host's home"),
            );
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (c::primary(ui, available, "Open").clicked() || enter) && available {
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
        egui::ScrollArea::both()
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
                if c::quiet(ui, true, "Copy path").clicked() {
                    ui.ctx().copy_text(selected.clone());
                }
            });
        }
        if c::quiet(ui, available && browser.more, "Load more").clicked() {
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
        ui.heading("Transfers");
        c::description(
            ui,
            "Encrypted downloads from the USB computer, verified before saving.",
        );
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
        if c::primary(
            ui,
            available
                && !self.snapshot.downloads.active()
                && self.snapshot.downloads.session != Session::Negotiating,
            "Connect encrypted host session",
        )
        .clicked()
        {
            command = Some(Command::ConnectSession);
        }
        ui.add_space(8.0);
        ui.label("Host source path");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.download_path)
                    .char_limit(512)
                    .desired_width(f32::INFINITY),
            );
        });
        if c::quiet(ui, true, "Use selected filesystem path").clicked()
            && let Some(path) = &self.snapshot.filesystem.selected
        {
            self.download_path = path.clone();
        }
        ui.label("Save to local filename");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.download_destination)
                    .char_limit(4096)
                    .desired_width((ui.available_width() - 100.0).max(80.0)),
            );
            if c::quiet(ui, !self.desktop_busy(), "Choose…").clicked() {
                self.destination_error = self
                    .desktop
                    .destination(
                        crate::desktop::DestinationContext::from_snapshot(&self.snapshot),
                        self.download_destination.clone(),
                    )
                    .err();
            }
        });
        if let Some(error) = self.destination_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let destination = std::path::PathBuf::from(&self.download_destination);
        let ready = available && self.snapshot.downloads.session == Session::Ready;
        let queue = Command::Queue {
            path: self.download_path.clone(),
            destination: destination.clone(),
        };
        let default = Command::QueueDefault { destination };
        ui.horizontal_wrapped(|ui| {
            if c::primary(ui, ready && queue.valid(), "Queue file").clicked() {
                command = Some(queue);
            }
            if c::quiet(
                ui,
                ready && Command::SetDefault(self.download_path.clone()).valid(),
                "Set host default path",
            )
            .clicked()
            {
                command = Some(Command::SetDefault(self.download_path.clone()));
            }
            if c::quiet(ui, ready && default.valid(), "Queue host default").clicked() {
                command = Some(default);
            }
            if c::quiet(
                ui,
                available && self.snapshot.downloads.active(),
                "Cancel downloads",
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

    fn diagnostics(&mut self, ui: &mut egui::Ui) {
        c::section(ui, "Activity");
        ui.horizontal_wrapped(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.diagnostic_filter)
                    .char_limit(128)
                    .desired_width(180.0),
            );
            ui.checkbox(&mut self.errors_only, "Errors only");
            ui.checkbox(&mut self.autoscroll, "Autoscroll");
            if c::quiet(ui, true, "Clear").clicked() {
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

impl CompanionApp {
    fn poll_state(&mut self) {
        self.poll_desktop();
        if self.backend.snapshots.has_changed().unwrap_or(false) {
            self.snapshot = self.backend.snapshots.borrow_and_update().clone();
            if self.snapshot.connection == Connection::Connected {
                self.selected = self
                    .snapshot
                    .selected
                    .as_ref()
                    .map(|device| device.id.clone());
            } else if !matches!(
                self.snapshot.connection,
                Connection::Scanning | Connection::Connecting
            ) && self
                .selected
                .as_ref()
                .is_some_and(|id| !self.snapshot.devices.iter().any(|d| &d.id == id))
            {
                self.selected = None;
            }
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
        }
        if self.backend.scripts.has_changed().unwrap_or(false) {
            self.script = self.backend.scripts.borrow_and_update().clone();
        }
    }
}

impl eframe::App for CompanionApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_state();
        self.shell(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_bounds(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Rect> {
        match shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == needle => {
                Some(t.visual_bounding_rect())
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_bounds(s, needle)),
            _ => None,
        }
    }

    fn painted_fill(shape: &egui::epaint::Shape, point: egui::Pos2, color: egui::Color32) -> bool {
        match shape {
            egui::epaint::Shape::Rect(rect) => rect.fill == color && rect.rect.contains(point),
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().any(|s| painted_fill(s, point, color))
            }
            _ => false,
        }
    }

    fn frame(
        app: &mut CompanionApp,
        ctx: &egui::Context,
        mut events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1120.0, 760.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                app.poll_state();
                app.shell(ui);
            },
        );
        output.textures_delta.clear();
        output
    }

    fn click(app: &mut CompanionApp, ctx: &egui::Context, label: &str) {
        for _ in 0..2 {
            let _ = frame(app, ctx, vec![]);
        }
        let output = frame(app, ctx, vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|s| {
                text_bounds(&s.shape, label)
                    .filter(|r| s.clip_rect.contains(r.center()))
                    .map(|r| r.center())
            })
            .unwrap_or_else(|| panic!("Visible control missing: {label}"));
        for pressed in [true, false] {
            let _ = frame(
                app,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    fn wait_for(
        app: &mut CompanionApp,
        ctx: &egui::Context,
        ready: impl Fn(&CompanionApp) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let _ = frame(app, ctx, vec![]);
            if ready(app) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "UI state timed out: {:?} {:?}",
                app.snapshot.connection,
                app.snapshot.job
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn pointer_flow_requires_selection_then_control_and_stops_a_job_from_another_task() {
        let ctx = egui::Context::default();
        let mut app = CompanionApp::from_context(
            &ctx,
            Mode::Mock(companion_core::mock::Scenario::Normal),
            None,
        )
        .unwrap();
        wait_for(&mut app, &ctx, |app| {
            !app.snapshot.devices.is_empty() && !app.snapshot.pending
        });
        assert!(app.selected.is_none());
        click(&mut app, &ctx, "Devices");
        click(&mut app, &ctx, "Connect to selected Pico");
        assert_ne!(app.snapshot.connection, Connection::Connected);
        assert!(app.selected.is_none());
        click(&mut app, &ctx, "Pico Plus 2 W (simulated)");
        assert_eq!(app.selected.as_deref(), Some("mock-pico-1"));
        click(&mut app, &ctx, "Connect to selected Pico");
        wait_for(&mut app, &ctx, |app| {
            app.snapshot.connection == Connection::Connected
        });
        assert!(!app.snapshot.control_acquired);
        click(&mut app, &ctx, "Scripts");
        assert!(!app.script_start_allowed());
        click(&mut app, &ctx, "Start script");
        let _ = frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: Some(egui::Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
        );
        // Give the real backend time to expose an unintended start through either
        // path. The test must not spawn a Python child for the blocked demo.
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let _ = frame(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.script.run, 0);
        // The context action and toolbar use the same guarded acquisition path.
        click(&mut app, &ctx, "Acquire control");
        wait_for(&mut app, &ctx, |app| {
            app.snapshot.control_acquired && !app.snapshot.pending
        });
        assert!(app.script_start_allowed());
        click(&mut app, &ctx, "Keyboard");
        app.delay_ms = 5000;
        click(&mut app, &ctx, "Send text");
        wait_for(&mut app, &ctx, |app| app.snapshot.job == Job::Running);
        click(&mut app, &ctx, "Files");
        click(&mut app, &ctx, "Stop typing / script");
        wait_for(&mut app, &ctx, |app| {
            app.snapshot.job == Job::Finished(companion_core::Outcome::Cancelled)
        });
        assert_eq!(app.workspace, Workspace::Files);
        click(&mut app, &ctx, "Device ▾");
        click(&mut app, &ctx, "Release control");
        wait_for(&mut app, &ctx, |app| !app.snapshot.control_acquired);
        click(&mut app, &ctx, "Device ▾");
        click(&mut app, &ctx, "Disconnect");
        wait_for(&mut app, &ctx, |app| {
            app.snapshot.connection == Connection::Disconnected
        });
    }

    #[test]
    fn connected_devices_page_keeps_chrome_compact_and_only_shows_the_current_step() {
        let ctx = egui::Context::default();
        let mut app = CompanionApp::from_context(
            &ctx,
            Mode::Mock(companion_core::mock::Scenario::Normal),
            None,
        )
        .unwrap();
        app.snapshot = state::tests::ready();
        app.snapshot.control_acquired = false;
        app.workspace = Workspace::Devices;
        let mut output = None;
        for _ in 0..3 {
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1120.0, 760.0),
                    )),
                    ..Default::default()
                },
                |ui| app.shell(ui),
            );
            frame.textures_delta.clear();
            output = Some(frame);
        }
        let output = output.unwrap();
        assert!(
            output
                .shapes
                .iter()
                .any(|s| text_bounds(&s.shape, "Acquire control").is_some())
        );
        for stale in [
            "Connect to selected Pico",
            "Scan mock devices",
            "Release control",
        ] {
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|s| text_bounds(&s.shape, stale).is_some()),
                "Completed/inapplicable action still shown: {stale}"
            );
        }
        for id in ["connection-toolbar", "owned-work"] {
            let panel = egui::containers::panel::PanelState::load(&ctx, egui::Id::new(id)).unwrap();
            assert!(
                panel.size().y <= 64.0,
                "{id} consumed {} points",
                panel.size().y
            );
        }
    }

    #[test]
    fn stop_stays_visible_on_every_workspace_at_desktop_sizes_and_zoom() {
        let ctx = egui::Context::default();
        let mut app = CompanionApp::from_context(
            &ctx,
            Mode::Mock(companion_core::mock::Scenario::Normal),
            None,
        )
        .unwrap();
        app.snapshot = state::tests::ready();
        app.snapshot.job = Job::Running;
        app.snapshot.job_owner = Some(0);
        app.text = "Keep this unsent draft".into();
        for dark in [false, true] {
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            crate::ui::theme::apply(&ctx);
            for (width, height, zoom) in [
                (1120.0, 760.0, 1.0),
                (820.0, 790.0, 1.0),
                (480.0, 520.0, 1.0),
                (480.0, 520.0, 2.0),
            ] {
                ctx.set_zoom_factor(zoom);
                for workspace in Workspace::ALL {
                    app.workspace = workspace;
                    app.activity_open = true;
                    // Settle layout/fonts and panel changes before checking painted bounds.
                    let mut output = None;
                    for _ in 0..3 {
                        let mut frame = ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, height),
                                )),
                                ..Default::default()
                            },
                            |ui| app.shell(ui),
                        );
                        frame.textures_delta.clear();
                        output = Some(frame);
                    }
                    let output = output.unwrap();
                    let (clip, rect) = output
                        .shapes
                        .iter()
                        .find_map(|s| {
                            text_bounds(&s.shape, "Stop typing / script").map(|r| (s.clip_rect, r))
                        })
                        .expect("Stop must be painted on every workspace");
                    assert!(
                        clip.contains_rect(rect),
                        "Stop clipped: {workspace:?} {width}x{height} zoom={zoom} {clip:?} {rect:?}"
                    );
                    assert_eq!(app.text, "Keep this unsent draft");
                    assert_eq!(app.snapshot.job, Job::Running);
                    if width >= 800.0 && zoom == 1.0 {
                        let sidebar = egui::containers::panel::PanelState::load(
                            &ctx,
                            egui::Id::new("workspace-navigation"),
                        )
                        .unwrap()
                        .outer_rect;
                        let label = output
                            .shapes
                            .iter()
                            .find_map(|s| {
                                text_bounds(&s.shape, workspace.label())
                                    .filter(|r| sidebar.contains(r.center()))
                            })
                            .expect("Current workspace label missing");
                        let fill = if dark {
                            egui::Color32::from_rgb(38, 57, 79)
                        } else {
                            egui::Color32::from_rgb(220, 231, 250)
                        };
                        assert!(
                            output.shapes.iter().any(|s| painted_fill(
                                &s.shape,
                                label.center(),
                                fill
                            )),
                            "Current page lost its persistent background: {workspace:?}, dark={dark}"
                        );
                    }
                }
            }
        }
    }
}

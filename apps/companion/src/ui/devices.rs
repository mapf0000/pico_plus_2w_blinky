//! Explicit discovery selection and connection; never choose a device implicitly.
use super::*;

impl CompanionApp {
    pub(super) fn connection(&mut self, ui: &mut egui::Ui) {
        if self.snapshot.connection == Connection::Connected {
            theme::card(ui).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                c::badge(ui, "Connected", false);
                if let Some(device) = &self.snapshot.selected {
                    c::section(ui, &device.name);
                    egui::CollapsingHeader::new("Device details")
                        .id_salt("connected-device-details")
                        .show(ui, |ui| {
                            ui.label(format!("Device ID: {}", device.id));
                        });
                }
                if c::quiet(ui, true, "Change device…")
                    .on_hover_text("Disconnect this Pico and return to discovery")
                    .clicked()
                {
                    self.selected = None;
                    self.send(Action::Disconnect);
                }
            });
            ui.add_space(8.0);
            self.readiness(ui);
            if state::usb_readiness(&self.snapshot, self.has_profile()).ready
                && c::primary(ui, true, "Open Keyboard").clicked()
            {
                self.workspace = Workspace::Keyboard;
            }
            return;
        }
        let transitioning = matches!(
            self.snapshot.connection,
            Connection::Scanning | Connection::Connecting
        );
        c::section(ui, "Choose a Pico");
        c::description(ui, "Select a device from the list, then connect.");
        ui.horizontal_wrapped(|ui| {
            if c::quiet(
                ui,
                !self.desktop_busy() && !self.snapshot.pending,
                if self.mode.is_mock() {
                    "Scan mock devices"
                } else {
                    "Scan Bluetooth"
                },
            )
            .clicked()
            {
                self.selected = None;
                self.send(Action::Scan);
            }
            if transitioning {
                ui.spinner();
                ui.label(self.snapshot.connection.label());
                if c::quiet(ui, true, "Cancel").clicked() {
                    self.send(Action::Disconnect);
                }
            }
        });
        if self.snapshot.devices.is_empty() && !transitioning {
            if let Connection::Failed(error) = self.snapshot.connection {
                ui.label(error.message());
                c::description(
                    ui,
                    "Resolve the connection or Bluetooth permission issue, then scan again.",
                );
            } else {
                c::description(
                    ui,
                    if self.mode.is_mock() {
                        "No simulated devices found. Scan again."
                    } else {
                        "No devices found. Power the Pico and scan again."
                    },
                );
            }
        }
        for device in &self.snapshot.devices {
            let selected = self.selected.as_deref() == Some(&device.id);
            let p = theme::palette(ui);
            theme::card(ui)
                .fill(if selected { p.raised } else { p.surface })
                .show(ui, |ui| {
                    // Left justification keeps the marker and name together while
                    // the native radio response remains the full row's click target.
                    let response = ui
                        .add_enabled_ui(!transitioning, |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), 32.0),
                                egui::Layout::left_to_right(egui::Align::Center)
                                    .with_main_justify(true),
                                |ui| ui.add(egui::RadioButton::new(selected, &device.name)),
                            )
                            .inner
                        })
                        .inner;
                    if response.clicked() {
                        self.selected = Some(device.id.clone());
                    }
                    egui::CollapsingHeader::new("Device details")
                        .id_salt(("discovered-details", &device.id))
                        .show(ui, |ui| {
                            ui.label(format!("Device ID: {}", device.id));
                        });
                });
        }
        ui.add_space(12.0);
        if let Some(device) = state::selected_device(&self.snapshot, self.selected.as_deref()) {
            ui.strong(format!("Selected: {}", device.name));
        }
        let blocker = state::connect_blocker(
            &self.snapshot,
            self.selected.as_deref(),
            self.desktop_busy(),
        );
        if let Some(reason) = blocker {
            c::description(ui, reason);
        }
        if c::primary(ui, blocker.is_none(), "Connect to selected Pico").clicked()
            && let Some(id) = &self.selected
        {
            self.send(Action::Connect(id.clone()));
        }
        if !transitioning
            && !self.desktop_busy()
            && !self.snapshot.pending
            && self.snapshot.selected.is_some()
            && c::quiet(ui, true, "Reconnect previous Pico").clicked()
        {
            self.send(Action::Reconnect);
        }
    }
}

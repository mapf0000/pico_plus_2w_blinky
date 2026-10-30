//! Device-scoped connection actions; discovery never chooses a Pico implicitly.
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
        c::section(ui, "Nearby Picos");
        c::description(ui, "Choose Connect beside the Pico you want to use.");
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
            } else if !self.snapshot.devices.is_empty() {
                let count = self.snapshot.devices.len();
                ui.small(format!(
                    "{count} {} found",
                    if count == 1 { "device" } else { "devices" }
                ));
            }
        });
        if let Connection::Failed(error) = self.snapshot.connection {
            ui.colored_label(theme::palette(ui).error, error.message());
        }
        if self.snapshot.devices.is_empty() && !transitioning {
            if matches!(self.snapshot.connection, Connection::Failed(_)) {
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
        let mut connect = None;
        for device in &self.snapshot.devices {
            let blocker =
                state::connect_blocker(&self.snapshot, Some(&device.id), self.desktop_busy());
            let connecting = self.snapshot.connection == Connection::Connecting
                && self
                    .snapshot
                    .selected
                    .as_ref()
                    .is_some_and(|d| d.id == device.id);
            ui.push_id(("nearby-pico", &device.id), |ui| {
                theme::card(ui)
                    .inner_margin(12)
                    .stroke(egui::Stroke::new(1.0, theme::palette(ui).divider))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        let narrow = ui.available_width() < 340.0;
                        let info = |ui: &mut egui::Ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.add(
                                egui::Label::new(egui::RichText::new(&device.name).strong()).wrap(),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!("ID: {}", device.id))
                                        .small()
                                        .color(theme::palette(ui).secondary),
                                )
                                .wrap(),
                            );
                        };
                        let button = |ui: &mut egui::Ui| {
                            // Stable width and ID retain the target during connection updates.
                            let response = ui
                                .push_id("connect", |ui| {
                                    c::primary_sized(
                                        ui,
                                        blocker.is_none(),
                                        if connecting {
                                            "Connecting…"
                                        } else {
                                            "Connect"
                                        },
                                        egui::vec2(128.0, 32.0),
                                    )
                                })
                                .inner;
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    blocker.is_none(),
                                    format!("Connect to {} (ID: {})", device.name, device.id),
                                )
                            });
                            let response = response.on_hover_text(blocker.map_or_else(
                                || format!("Connect to {}", device.name),
                                str::to_owned,
                            ));
                            if response.clicked() {
                                connect = Some(device.id.clone());
                            }
                        };
                        if narrow {
                            ui.vertical(info);
                            ui.scope(button);
                        } else {
                            ui.horizontal(|ui| {
                                let width =
                                    ui.available_width() - 128.0 - ui.spacing().item_spacing.x;
                                ui.allocate_ui_with_layout(
                                    egui::vec2(width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_min_width(width);
                                        info(ui);
                                    },
                                );
                                ui.scope(button);
                            });
                        }
                    });
            });
        }
        if self.desktop_busy() {
            c::description(
                ui,
                "Finish the profile or local dialog operation before connecting.",
            );
        } else if self.snapshot.pending && !transitioning {
            c::description(ui, "Waiting for the current command to finish.");
        }
        if let Some(id) = connect {
            self.selected = Some(id.clone());
            self.send(Action::Connect(id));
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

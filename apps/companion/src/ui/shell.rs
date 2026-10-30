//! Desktop panel composition. Child module of app so drafts stay private.
use super::*;

impl CompanionApp {
    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
        let compact = ui.available_width() < 800.0;
        let short = ui.available_height() < 650.0;
        // The first panel stays outermost: Stop never moves into scrolling content.
        egui::Panel::bottom("owned-work")
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::chrome(ui))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let active = self.script.state.active();
                    let cancel = state::cancel_manual(&self.snapshot, active, self.script.run);
                    if c::stop(ui, active || cancel, "Stop typing / script").clicked() {
                        if active {
                            self.enqueue_error = self.backend.stop_script(self.script.run).err();
                        }
                        if cancel {
                            self.send(Action::Cancel);
                        }
                    }
                    if self.snapshot.downloads.active()
                        && c::quiet(ui, true, "Cancel downloads").clicked()
                    {
                        self.send(Action::Download(companion_core::download::Command::Cancel));
                    }
                    let label = if active {
                        self.script.state.label()
                    } else if self.snapshot.job == Job::Idle {
                        "Keyboard idle"
                    } else {
                        state::job_label(self.snapshot.job)
                    };
                    ui.add_sized(
                        [if compact { 120.0 } else { 230.0 }, 24.0],
                        egui::Label::new(egui::RichText::new(label).small()).truncate(),
                    )
                    .on_hover_text(label);
                    let errors = self
                        .snapshot
                        .diagnostics
                        .iter()
                        .filter(|d| d.error.is_some())
                        .count();
                    let label = if errors == 0 {
                        "Activity".into()
                    } else {
                        format!("Activity ({errors})")
                    };
                    if c::quiet(ui, self.workspace != Workspace::Activity, &label).clicked() {
                        if short {
                            self.workspace = Workspace::Activity;
                        } else {
                            self.activity_open = !self.activity_open;
                        }
                    }
                });
            });
        egui::Panel::top("connection-toolbar")
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::chrome(ui))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.mode.is_mock() {
                        c::badge(ui, "Mock", false)
                            .on_hover_text("Simulated device · no Bluetooth or USB access");
                    }
                    let device = if matches!(
                        self.snapshot.connection,
                        Connection::Connected | Connection::Connecting
                    ) {
                        self.snapshot.selected.as_ref()
                    } else {
                        state::selected_device(&self.snapshot, self.selected.as_deref())
                    };
                    let name = device.map_or("Choose a Pico", |d| d.name.as_str());
                    ui.add_sized(
                        [if compact { 160.0 } else { 210.0 }, 24.0],
                        egui::Label::new(egui::RichText::new(name).strong()).truncate(),
                    )
                    .on_hover_text(name);
                    c::badge(ui, self.snapshot.connection.label(), false);
                    if self.snapshot.connection == Connection::Connected {
                        c::badge(
                            ui,
                            if self.snapshot.control_acquired {
                                "Control acquired"
                            } else {
                                "Control needed"
                            },
                            !self.snapshot.control_acquired,
                        );
                        if let Some(caps) = &self.snapshot.capabilities {
                            c::badge(
                                ui,
                                if caps.status.usb_ready {
                                    "USB ready"
                                } else if caps.status.usb_enabled {
                                    "USB waiting"
                                } else {
                                    "USB off"
                                },
                                !caps.status.usb_ready,
                            );
                        }
                        if let Some(action) =
                            state::usb_readiness(&self.snapshot, self.has_profile()).action
                            && action != state::ReadinessAction::Devices
                        {
                            self.readiness_action(ui, action);
                        }
                    }
                    if c::quiet(ui, true, "Devices").clicked() {
                        self.workspace = Workspace::Devices;
                    }
                    if matches!(
                        self.snapshot.connection,
                        Connection::Connected | Connection::Connecting | Connection::Scanning
                    ) {
                        let (response, _) = egui::containers::menu::MenuButton::from_button(
                            egui::Button::new("Device ▾").frame(false),
                        )
                        .ui(ui, |ui| {
                            if self.snapshot.control_acquired
                                && c::quiet(
                                    ui,
                                    !self.snapshot.pending && !self.snapshot.job.active(),
                                    "Release control",
                                )
                                .clicked()
                            {
                                self.send(Action::Release);
                                ui.close();
                            }
                            if c::quiet(ui, true, "Disconnect").clicked() {
                                self.send(Action::Disconnect);
                                ui.close();
                            }
                        });
                        c::focus(ui, &response, theme::palette(ui).accent);
                    }
                });
                if let Some(error) = self.enqueue_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(error) = self.desktop_error {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                        if c::quiet(ui, true, "Dismiss").clicked() {
                            self.desktop_error = None;
                        }
                    });
                }
                if let Some(error) = self.snapshot.last_error {
                    ui.colored_label(ui.visuals().error_fg_color, error.message());
                }
                if compact {
                    egui::ComboBox::from_id_salt("workspace-selector")
                        .selected_text(self.workspace.label())
                        .show_ui(ui, |ui| self.navigation(ui));
                }
            });
        if self.activity_open && !short && self.workspace != Workspace::Activity {
            egui::Panel::bottom("activity-drawer")
                .resizable(true)
                .default_size(220.0)
                .min_size(130.0)
                .max_size(280.0)
                .frame(theme::chrome(ui))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("drawer-content")
                        .show(ui, |ui| self.diagnostics(ui));
                });
        }
        if !compact {
            egui::Panel::left("workspace-navigation")
                .resizable(false)
                .exact_size(184.0)
                .show_separator_line(false)
                .frame(theme::chrome(ui))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sidebar-tasks")
                        .show(ui, |ui| self.navigation(ui));
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::palette(ui).canvas)
                    .inner_margin(if compact { 16 } else { 24 }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("workspace", self.workspace.label()))
                    .show(ui, |ui| self.workspace_content(ui));
            });
    }

    fn workspace_content(&mut self, ui: &mut egui::Ui) {
        let max_width = if matches!(
            self.workspace,
            Workspace::Files | Workspace::Scripts | Workspace::Transfers | Workspace::Activity
        ) {
            f32::INFINITY
        } else {
            760.0
        };
        // A reading-width limit must not expand content past a narrow viewport.
        ui.set_max_width(ui.available_width().min(max_width));
        match self.workspace {
            Workspace::Overview => {
                self.status(ui);
                ui.separator();
                self.usb_controls(ui);
                self.readiness(ui);
                if ui.button("Open Keyboard").clicked() {
                    self.workspace = Workspace::Keyboard;
                }
            }
            Workspace::Devices => {
                ui.heading("Devices");
                self.profiles(ui);
                c::description(ui, "Connect a Pico and grant control to its USB tools.");
                ui.add_space(12.0);
                self.connection(ui);
            }
            Workspace::Keyboard => self.keyboard(ui),
            Workspace::Scripts => self.scripting(ui),
            Workspace::Files => self.filesystem(ui),
            Workspace::Transfers => self.downloads(ui),
            Workspace::Settings => {
                ui.heading("Settings");
                ui.label("Appearance");
                egui::widgets::global_theme_preference_buttons(ui);
                ui.small("Appearance follows the system by default. Preferences are kept for this launch only.");
                ui.separator();
                self.usb_identity(ui);
            }
            Workspace::Help => {
                ui.heading("Help");
                ui.label(
                    "1. In Devices, import your private profile, scan, then choose Connect beside your Pico.",
                );
                ui.label(
                    "2. Choose Acquire control after connecting. No Pico confirmation is needed.",
                );
                ui.label("3. Enable USB in Overview or Settings. Keyboard actions type on the computer connected to Pico USB.");
                ui.label("4. Choose the USB computer's input layout and a delay before sending text to a disposable editor.");
                ui.label("Stop typing / script is always at the bottom of this window. Pico Y stops device-wide execution. Cancel downloads is separate.");
                ui.label("Reconnect does not acquire control or replay effects. Changing a profile requires disconnecting and stopping Python.");
                ui.label("Scripts: Ctrl/Cmd+Enter starts or restarts the visible script. Closing the app stops its work.");
                ui.label("Files and Transfers require USB and the host agent. Downloads are verified and never overwrite an existing local file.");
            }
            Workspace::Activity => self.diagnostics(ui),
        }
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        for workspace in Workspace::ALL {
            if workspace == Workspace::Settings {
                ui.add_space(20.0);
            }
            ui.scope(|ui| {
                let p = theme::palette(ui);
                ui.visuals_mut().selection.bg_fill = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(38, 57, 79)
                } else {
                    egui::Color32::from_rgb(220, 231, 250)
                };
                ui.visuals_mut().selection.stroke.color = p.primary;
                let response = ui
                    .allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 34.0),
                        egui::Layout::left_to_right(egui::Align::Center).with_main_justify(true),
                        |ui| {
                            ui.add(
                                egui::Button::selectable(
                                    self.workspace == workspace,
                                    workspace.label(),
                                )
                                .frame_when_inactive(self.workspace == workspace)
                                .stroke(egui::Stroke::NONE),
                            )
                        },
                    )
                    .inner;
                if self.workspace == workspace {
                    let marker = egui::Rect::from_min_max(
                        response.rect.left_top() + egui::vec2(3.0, 8.0),
                        response.rect.left_bottom() + egui::vec2(6.0, -8.0),
                    );
                    ui.painter().rect_filled(marker, 2, p.accent);
                }
                c::focus(ui, &response, p.accent);
                if response.clicked() {
                    self.workspace = workspace;
                }
            });
        }
    }
}

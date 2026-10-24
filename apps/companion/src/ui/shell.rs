//! Desktop panel composition. Child module of app so drafts stay private.
use super::*;

impl CompanionApp {
    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
        let compact = ui.available_width() < 800.0;
        let short = ui.available_height() < 650.0;
        // First/outermost bottom panel: Stop never belongs to a scroll region.
        egui::Panel::bottom("owned-work")
            .resizable(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let script_active = self.script.state.active();
                    let cancel_manual =
                        state::cancel_manual(&self.snapshot, script_active, self.script.run);
                    if ui
                        .add_enabled(
                            script_active || cancel_manual,
                            egui::Button::new("Stop typing / script"),
                        )
                        .clicked()
                    {
                        if script_active {
                            self.enqueue_error = self.backend.stop_script(self.script.run).err();
                        }
                        if cancel_manual {
                            self.send(Action::Cancel);
                        }
                    }
                    if self.snapshot.downloads.active() && ui.button("Cancel downloads").clicked() {
                        self.send(Action::Download(companion_core::download::Command::Cancel));
                    }
                    ui.label(if script_active {
                        self.script.state.label()
                    } else {
                        state::job_label(self.snapshot.job)
                    });
                });
            });
        egui::Panel::top("connection-toolbar")
            .resizable(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Pico Companion");
                    if self.mode.is_mock() {
                        ui.label("MOCK · no hardware access");
                    }
                    let name = self
                        .snapshot
                        .selected
                        .as_ref()
                        .map_or("No Pico selected", |d| d.name.as_str());
                    ui.add(egui::Label::new(name).truncate())
                        .on_hover_text(name);
                    ui.label(self.snapshot.connection.label());
                    ui.label(if self.snapshot.control_acquired {
                        "Control acquired"
                    } else {
                        "Control released"
                    });
                    ui.label(
                        self.snapshot
                            .capabilities
                            .as_ref()
                            .map_or("USB unknown", |c| {
                                if c.status.usb_ready {
                                    "USB ready"
                                } else if c.status.usb_enabled {
                                    "USB waiting for host"
                                } else {
                                    "USB disabled"
                                }
                            }),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Devices").clicked() {
                        self.workspace = Workspace::Devices;
                    }
                    if matches!(
                        self.snapshot.connection,
                        Connection::Connected | Connection::Connecting | Connection::Scanning
                    ) && ui.button("Disconnect").clicked()
                    {
                        self.send(Action::Disconnect);
                    }
                });
                if self.snapshot.connection == Connection::Connected {
                    self.control_actions(ui);
                }
                if let Some(error) = self.enqueue_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(error) = self.desktop_error {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                        if ui.button("Dismiss").clicked() {
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
                        .show_ui(ui, |ui| {
                            self.navigation(ui);
                        });
                }
            });
        egui::Panel::bottom("activity-toggle")
            .resizable(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let errors = self
                        .snapshot
                        .diagnostics
                        .iter()
                        .filter(|d| d.error.is_some())
                        .count();
                    ui.label(format!("Activity · {errors} errors in retained history"));
                    if self.workspace != Workspace::Activity
                        && ui
                            .button(if self.activity_open && !short {
                                "Hide activity"
                            } else {
                                "Show activity"
                            })
                            .clicked()
                    {
                        if short {
                            self.workspace = Workspace::Activity;
                        } else {
                            self.activity_open = !self.activity_open;
                        }
                    }
                });
            });
        if self.activity_open && !short && self.workspace != Workspace::Activity {
            egui::Panel::bottom("activity-drawer")
                .resizable(true)
                .default_size(220.0)
                .min_size(130.0)
                .max_size(280.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("drawer-content")
                        .show(ui, |ui| self.diagnostics(ui));
                });
        }
        if !compact {
            egui::Panel::left("workspace-navigation")
                .resizable(false)
                .exact_size(if ui.available_width() >= 1100.0 {
                    200.0
                } else {
                    176.0
                })
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sidebar-tasks")
                        .show(ui, |ui| self.navigation(ui));
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("workspace", self.workspace.label()))
                .show(ui, |ui| self.workspace_content(ui));
        });
    }

    fn workspace_content(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(
            if matches!(
                self.workspace,
                Workspace::Files | Workspace::Scripts | Workspace::Transfers | Workspace::Activity
            ) {
                f32::INFINITY
            } else {
                760.0
            },
        );
        match self.workspace {
            Workspace::Overview => {
                self.status(ui);
                ui.separator();
                self.usb_controls(ui);
                if let Some(reason) =
                    state::typing_blocker(&self.snapshot, self.script.state.active(), &self.layout)
                {
                    ui.label(reason);
                }
                if ui.button("Open Keyboard").clicked() {
                    self.workspace = Workspace::Keyboard;
                }
            }
            Workspace::Devices => {
                ui.heading("Devices");
                self.profiles(ui);
                ui.separator();
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
                    "1. In Devices, import your private profile, scan, select a Pico and connect.",
                );
                ui.label("2. Authenticate and acquire control using the connection toolbar. No Pico confirmation is needed.");
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
                ui.separator();
            }
            ui.selectable_value(&mut self.workspace, workspace, workspace.label());
        }
    }
}

//! Script controls and recovery, without interpreter work in UI callbacks.
use super::*;

impl CompanionApp {
    pub(super) fn script_start_allowed(&self) -> bool {
        state::script_start_allowed(
            &self.snapshot,
            self.script.run,
            self.script.state.active(),
            self.script_source == crate::python::DEMO,
            python_runtime::source_limit_error(&self.script_source).is_none(),
        )
    }
    pub(super) fn scripting(&mut self, ui: &mut egui::Ui) {
        ui.heading("Scripts");
        c::description(
            ui,
            "Python runs here and controls the USB computer. Edits apply to your next run.",
        );
        self.readiness(ui);
        if self.script_source == crate::python::DEMO {
            ui.small("USB typing demo · acquire control and enable USB before Start. The demo waits five seconds before typing.");
        } else {
            ui.small("Custom scripts can start offline to wait for events. USB effects require acquired control and a ready keyboard.");
        }
        ui.add(
            egui::TextEdit::multiline(&mut self.script_source)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .desired_rows(10)
                .desired_width(f32::INFINITY)
                .char_limit(python_runtime::MAX_SOURCE_BYTES),
        );
        let invalid = python_runtime::source_limit_error(&self.script_source);
        let available = self.script_start_allowed();
        let shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        let mut start = shortcut && available;
        ui.horizontal_wrapped(|ui| {
            if c::primary(
                ui,
                available,
                if self.script.state.active() {
                    "Restart script"
                } else {
                    "Start script"
                },
            )
            .clicked()
            {
                start = true;
            }
            if c::stop(ui, self.script.state.active(), "Stop script").clicked() {
                self.enqueue_error = self.backend.stop_script(self.script.run).err();
            }
            if c::quiet(ui, true, "Use Python demo").clicked() {
                self.script_source = crate::python::DEMO.into();
            }
            ui.label("Ctrl/Cmd+Enter to start");
        });
        if start && self.script_start_allowed() {
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
        egui::CollapsingHeader::new("Run details")
            .id_salt("script-run-details")
            .show(ui, |ui| {
                if let Some(layout) = &self.script.layout {
                    ui.label(format!("Script input layout: {layout}"));
                }
                if self.script.heap_peak != 0 {
                    ui.small(format!(
                        "Peak Python Rust heap: {:.1} MiB / 128 MiB",
                        self.script.heap_peak as f64 / 1_048_576.0
                    ));
                }
            });
        if let Some(error) = invalid {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(error) = &self.script.error {
            let (title, advice) = state::script_error_advice(error);
            ui.colored_label(ui.visuals().error_fg_color, title);
            ui.label(advice);
            egui::CollapsingHeader::new("Technical details")
                .id_salt("python-error-details")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("python-error")
                        .max_height(150.0)
                        .show(ui, |ui| {
                            ui.monospace(error);
                        });
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
}

//! Small native egui controls with consistent emphasis and focus treatment.
use super::theme;
use eframe::egui;

pub fn primary(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    primary_sized(ui, enabled, label, egui::Vec2::ZERO)
}

pub fn primary_sized(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    min_size: egui::Vec2,
) -> egui::Response {
    filled(ui, enabled, label, false, min_size)
}

pub fn stop(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    if enabled {
        filled(ui, true, label, true, egui::Vec2::ZERO)
    } else {
        quiet(ui, false, label)
    }
}

fn filled(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    danger: bool,
    min_size: egui::Vec2,
) -> egui::Response {
    ui.scope(|ui| {
        let accent = theme::palette(ui).accent;
        let visuals = ui.visuals_mut();
        let colors = if danger {
            [0xb32635, 0x9d2030, 0x831a28]
        } else {
            [0x245ccd, 0x1f50b5, 0x1b4294]
        };
        for (widget, color) in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ]
        .into_iter()
        .zip(colors)
        {
            widget.bg_fill =
                egui::Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8);
            widget.weak_bg_fill = widget.bg_fill;
            widget.fg_stroke.color = egui::Color32::WHITE;
            widget.bg_stroke = egui::Stroke::NONE;
            widget.corner_radius = egui::CornerRadius::same(6);
        }
        let response = ui.add_enabled(enabled, egui::Button::new(label).min_size(min_size));
        focus(ui, &response, accent);
        response
    })
    .inner
}

pub fn quiet(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    ui.scope(|ui| {
        let p = theme::palette(ui);
        let visuals = ui.visuals_mut();
        visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
        visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
        ] {
            widget.bg_stroke = egui::Stroke::NONE;
            widget.corner_radius = egui::CornerRadius::same(6);
        }
        visuals.widgets.hovered.bg_fill = p.raised;
        visuals.widgets.hovered.weak_bg_fill = p.raised;
        visuals.widgets.active.bg_fill = p.raised;
        visuals.widgets.active.weak_bg_fill = p.raised;
        let response = ui.add_enabled(enabled, egui::Button::new(label));
        focus(ui, &response, p.accent);
        response
    })
    .inner
}

pub fn focus(ui: &egui::Ui, response: &egui::Response, color: egui::Color32) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            7,
            egui::Stroke::new(2.0, color),
            egui::StrokeKind::Outside,
        );
    }
}

pub fn badge(ui: &mut egui::Ui, text: &str, attention: bool) -> egui::Response {
    let p = theme::palette(ui);
    egui::Frame::new()
        .fill(p.raised)
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(if attention {
                p.warning
            } else {
                if matches!(text, "Ready" | "USB ready" | "Control acquired") {
                    p.success
                } else {
                    p.primary
                }
            }))
        })
        .inner
}

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(18.0).strong());
}

pub fn description(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::palette(ui).secondary));
}

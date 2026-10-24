use eframe::egui;

pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let colors = Palette::new(style.visuals.dark_mode);
        let visuals = &mut style.visuals;
        visuals.panel_fill = colors.surface;
        visuals.window_fill = colors.surface;
        visuals.extreme_bg_color = colors.input;
        visuals.faint_bg_color = colors.raised;
        visuals.weak_text_color = Some(colors.secondary);
        visuals.error_fg_color = colors.error;
        visuals.warn_fg_color = colors.warning;
        visuals.hyperlink_color = colors.accent;
        visuals.selection.bg_fill = egui::Color32::from_rgb(36, 92, 205);
        visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.fg_stroke.color = colors.primary;
            widget.bg_stroke = egui::Stroke::new(1.0, colors.boundary);
        }
        visuals.widgets.inactive.bg_fill = colors.raised;
        visuals.widgets.inactive.weak_bg_fill = colors.raised;
        visuals.widgets.hovered.bg_fill = colors.raised;
        visuals.widgets.hovered.weak_bg_fill = colors.raised;
        visuals.widgets.hovered.bg_stroke.color = colors.accent;
        visuals.widgets.active.bg_fill = colors.raised;
        visuals.widgets.active.weak_bg_fill = colors.raised;
        visuals.widgets.active.bg_stroke.color = colors.accent;
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 32.0;
        for (kind, size) in [
            (egui::TextStyle::Heading, 24.0),
            (egui::TextStyle::Body, 15.0),
            (egui::TextStyle::Button, 15.0),
            (egui::TextStyle::Small, 13.0),
            (egui::TextStyle::Monospace, 14.0),
        ] {
            let family = if kind == egui::TextStyle::Monospace {
                egui::FontFamily::Monospace
            } else {
                egui::FontFamily::Proportional
            };
            style
                .text_styles
                .insert(kind, egui::FontId::new(size, family));
        }
    });
}

struct Palette {
    surface: egui::Color32,
    input: egui::Color32,
    raised: egui::Color32,
    primary: egui::Color32,
    secondary: egui::Color32,
    boundary: egui::Color32,
    accent: egui::Color32,
    error: egui::Color32,
    warning: egui::Color32,
}
impl Palette {
    fn new(dark: bool) -> Self {
        let rgb = |value: u32| {
            egui::Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
        };
        let values = if dark {
            [
                0x1c2330, 0x121923, 0x252e3d, 0xedf2f9, 0xb6c2d4, 0x7c8ea8, 0x94baff, 0xffa6ad,
                0xf0c879,
            ]
        } else {
            [
                0xffffff, 0xffffff, 0xe9eef5, 0x172033, 0x4b5b70, 0x687a94, 0x1d4ed8, 0xa91d34,
                0x805200,
            ]
        };
        Self {
            surface: rgb(values[0]),
            input: rgb(values[1]),
            raised: rgb(values[2]),
            primary: rgb(values[3]),
            secondary: rgb(values[4]),
            boundary: rgb(values[5]),
            accent: rgb(values[6]),
            error: rgb(values[7]),
            warning: rgb(values[8]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(c: egui::Color32) -> f64 {
        let linear = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    }
    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn foreground_and_field_tokens_meet_contrast_targets() {
        for dark in [false, true] {
            let p = Palette::new(dark);
            for surface in [p.surface, p.input, p.raised] {
                for text in [p.primary, p.secondary, p.accent, p.error, p.warning] {
                    assert!(contrast(text, surface) >= 4.5, "dark={dark} text={text:?}");
                }
                assert!(contrast(p.boundary, surface) >= 3.0);
            }
        }
        assert!(contrast(egui::Color32::WHITE, egui::Color32::from_rgb(36, 92, 205)) >= 4.5);
    }
}

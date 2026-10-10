//! Bounded text-row scenes. Shared widgets keep page layout and drawing consistent.

use embedded_graphics::{
    mono_font::ascii::{FONT_6X9, FONT_8X13},
    prelude::*,
    primitives::Rectangle,
};
use firmware_exec::jobs::Phase;
use heapless::Vec;

use super::{
    model::{AgentAction, KeyboardSnapshot, PageView, Severity, Status, Text, bounded_text},
    renderer::Palette,
};

pub const MAX_ROWS: usize = 20;
pub const MAX_LOG_ROWS: usize = 18;
pub const TEXT_PAD: u32 = 6;
const _: () = assert!(MAX_ROWS > MAX_LOG_ROWS);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Body,
    Title,
}

impl Font {
    pub fn metrics(self) -> &'static embedded_graphics::mono_font::MonoFont<'static> {
        match self {
            Self::Body => &FONT_6X9,
            Self::Title => &FONT_8X13,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub rect: Rectangle,
    pub text: Text,
    pub font: Font,
    pub foreground: embedded_graphics::pixelcolor::Rgb565,
    pub background: embedded_graphics::pixelcolor::Rgb565,
}

pub struct Scene {
    pub rows: Vec<Row, MAX_ROWS>,
    viewport: Rectangle,
    cursor: i32,
    palette: Palette,
}

/// The ASCII fonts have fixed cell widths. Unsupported/control characters are
/// normalized before caching so truncation and cached comparisons are stable.
pub fn fit_text(text: &str, columns: usize) -> Text {
    let columns = columns.min(super::model::TEXT_CAPACITY);
    let truncated = text.chars().count() > columns;
    let suffix = if truncated { columns.min(3) } else { 0 };
    let mut out = Text::new();
    for character in text.chars().take(columns - suffix) {
        let character = if character.is_ascii() && !character.is_control() {
            character
        } else {
            '?'
        };
        // Each normalized character uses one byte; the loop is bounded by capacity.
        let _ = out.push(character);
    }
    for _ in 0..suffix {
        let _ = out.push('.');
    }
    out
}

impl Scene {
    pub fn build(view: PageView<'_>, viewport: Rectangle, palette: Palette) -> Self {
        let title = view.id().title();
        let mut scene = Self {
            rows: Vec::new(),
            viewport,
            cursor: viewport.top_left.y + 8,
            palette,
        };
        scene.row(title, 18, Font::Title, palette.white, palette.teal);
        scene.cursor += 4;
        match view {
            PageView::Payloads {
                model,
                presets,
                keyboard,
            } => {
                scene.body("A/B: Select, X: Run, A+X: Menu", 12);
                scene.body("Y: Stop active keyboard job", 12);
                let keyboard = match keyboard {
                    KeyboardSnapshot::Unavailable => {
                        bounded_text(format_args!("Keyboard: USB unavailable"))
                    }
                    KeyboardSnapshot::Ready => bounded_text(format_args!("Keyboard: ready")),
                    KeyboardSnapshot::Busy {
                        local,
                        phase,
                        cancelled,
                    } => {
                        let owner = if local { "local preset" } else { "companion" };
                        let phase = if cancelled {
                            "stopping"
                        } else {
                            match phase {
                                Phase::Queued => "queued",
                                Phase::Running => "running",
                                Phase::Releasing => "releasing keys",
                            }
                        };
                        bounded_text(format_args!("Keyboard: {owner}, {phase}"))
                    }
                };
                scene.body(&keyboard, 14);
                scene.cursor += 4;
                // Footer space is reserved before calculating the list viewport.
                let bottom = viewport.top_left.y + viewport.size.height as i32;
                let footer = bottom - 52;
                let rows = ((footer - scene.cursor).max(0) as usize / 16).min(MAX_ROWS - 8);
                for index in model.selected.window(presets.len(), rows) {
                    let preset = &presets[index];
                    scene.action(preset.name, index == model.selected.0, preset.available);
                }
                if presets.is_empty() {
                    scene.body("No keyboard presets", 16);
                }
                scene.cursor = footer.max(scene.cursor);
                if let Some(preset) = presets.get(model.selected.0) {
                    scene.body(
                        &bounded_text(format_args!("Input layout: {}", preset.layout)),
                        12,
                    );
                    let detail = if !preset.available {
                        "Unavailable: package host agent first"
                    } else if preset.launches_agent {
                        "Opens Terminal; copies and starts agent"
                    } else {
                        "Types Hello from Pico! in focused app"
                    };
                    scene.body(detail, 12);
                } else {
                    scene.body("", 12);
                    scene.body("", 12);
                }
                scene.status(&model.status);
            }
            PageView::Transfer(model) => {
                scene.status_or_help(&model.status, "A/B: Select, X: Act, A+X: Menu");
                let snapshot = model.snapshot;
                let usb = if snapshot.usb_ready {
                    "ready"
                } else {
                    "waiting"
                };
                let browser = if snapshot.browser_connected {
                    "connected"
                } else {
                    "none"
                };
                let mode = if snapshot.simulation {
                    "simulation(drop)"
                } else {
                    "relay->browser"
                };
                scene.body(&bounded_text(format_args!("USB ctrl: {usb}")), 14);
                scene.body(&bounded_text(format_args!("Browser WS: {browser}")), 14);
                scene.body(&bounded_text(format_args!("Mode: {mode}")), 14);
                scene.body("Source: host default path", 14);
                scene.body(
                    &bounded_text(format_args!("State: {}", snapshot.state.name())),
                    14,
                );
                let id = if snapshot.id == 0 {
                    bounded_text(format_args!("Transfer id: -"))
                } else {
                    bounded_text(format_args!("Transfer id: {}", snapshot.id))
                };
                scene.body(&id, 14);
                scene.body(
                    &bounded_text(format_args!(
                        "Bytes: {} / {}",
                        snapshot.received_bytes, snapshot.total_bytes
                    )),
                    14,
                );
                scene.body(
                    &bounded_text(format_args!(
                        "Chunks: {} / {}",
                        snapshot.finished_chunks, snapshot.total_chunks
                    )),
                    14,
                );
                scene.cursor += 6;
                scene.action("Start in paired Web UI", model.selected.0 == 0, true);
                scene.action("Mode: Encrypted relay only", model.selected.0 == 1, true);
            }
            PageView::Agent(model) => {
                scene.status_or_help(&model.status, "A/B: Up/Down, X: Send, A+X: Menu");
                scene.cursor += 8;
                for (index, action) in AgentAction::ALL.iter().enumerate() {
                    scene.action(action.name(), index == model.selected.0, true);
                }
            }
            PageView::System { ssid, metrics } => {
                if let Some(code) = metrics.pairing_code {
                    scene.body("Bluetooth pairing", 20);
                    scene.body(&bounded_text(format_args!("Compare: {code:06}")), 24);
                    scene.body("X: Confirm, Y: Reject", 20);
                    return scene;
                }
                scene.body(&bounded_text(format_args!("Radio: {ssid}")), 14);
                let seconds = metrics.uptime_secs;
                let hours = seconds / 3600;
                let uptime = if hours >= 24 {
                    bounded_text(format_args!(
                        "Uptime: {}d {:02}:{:02}:{:02}",
                        hours / 24,
                        hours % 24,
                        (seconds / 60) % 60,
                        seconds % 60
                    ))
                } else {
                    bounded_text(format_args!(
                        "Uptime: {hours:02}:{:02}:{:02}",
                        (seconds / 60) % 60,
                        seconds % 60
                    ))
                };
                scene.body(&uptime, 14);
                scene.body(
                    &bounded_text(format_args!("CPU clock: {}MHz", metrics.cpu_mhz)),
                    14,
                );
                let temperature = match metrics.temperature_c {
                    Some(value) => bounded_text(format_args!("Temp: {value:.1}C")),
                    None => bounded_text(format_args!("Temp: read error")),
                };
                scene.body(&temperature, 14);
                scene.body(
                    &bounded_text(format_args!(
                        "SRAM: {} / {} KiB static",
                        metrics.sram_static_bytes.div_ceil(1024),
                        metrics.sram_total_bytes / 1024
                    )),
                    14,
                );
                scene.body(
                    &bounded_text(format_args!("PSRAM: {}", metrics.psram_status)),
                    14,
                );
                let used = metrics
                    .flash_total_bytes
                    .saturating_sub(metrics.flash_free_bytes);
                let free_percent = metrics
                    .flash_free_bytes
                    .saturating_mul(100)
                    .checked_div(metrics.flash_total_bytes)
                    .unwrap_or(0);
                scene.body(
                    &bounded_text(format_args!(
                        "Flash: {} / {} KiB used",
                        used.div_ceil(1024),
                        metrics.flash_total_bytes / 1024
                    )),
                    14,
                );
                scene.body(
                    &bounded_text(format_args!(
                        "Flash free: {} KiB ({free_percent}%)",
                        metrics.flash_free_bytes / 1024
                    )),
                    14,
                );
            }
            PageView::Logs(lines) => {
                let bottom = viewport.top_left.y + viewport.size.height as i32;
                let count = ((bottom - scene.cursor).max(0) as usize / 12).min(MAX_LOG_ROWS);
                for line in &lines[lines.len().saturating_sub(count)..] {
                    scene.body(line, 12);
                }
            }
        }
        scene
    }

    fn row(
        &mut self,
        text: &str,
        height: u32,
        font: Font,
        foreground: embedded_graphics::pixelcolor::Rgb565,
        background: embedded_graphics::pixelcolor::Rgb565,
    ) {
        let rect = Rectangle::new(
            Point::new(self.viewport.top_left.x, self.cursor),
            Size::new(self.viewport.size.width, height),
        );
        self.cursor += height as i32;
        if rect.intersection(&self.viewport) != rect || self.rows.is_full() {
            return;
        }
        let columns =
            rect.size.width.saturating_sub(TEXT_PAD * 2) / font.metrics().character_size.width;
        let _ = self.rows.push(Row {
            rect,
            text: fit_text(text, columns as usize),
            font,
            foreground,
            background,
        });
    }

    fn body(&mut self, text: &str, height: u32) {
        self.row(
            text,
            height,
            Font::Body,
            self.palette.white,
            self.palette.black,
        );
    }

    fn action(&mut self, text: &str, selected: bool, available: bool) {
        let foreground = if selected {
            self.palette.black
        } else if available {
            self.palette.white
        } else {
            self.palette.yellow
        };
        let background = if selected {
            self.palette.white
        } else {
            self.palette.black
        };
        self.row(text, 16, Font::Body, foreground, background);
    }

    fn status_or_help(&mut self, status: &Status, help: &str) {
        if status.text.is_empty() {
            self.body(help, 12);
            self.body("", 12);
        } else {
            self.status(status);
        }
    }

    fn status(&mut self, status: &Status) {
        let background = match status.severity {
            Severity::Normal => self.palette.black,
            Severity::Success => self.palette.green,
            Severity::Warning => self.palette.yellow,
        };
        let foreground = if status.severity == Severity::Normal {
            self.palette.white
        } else {
            self.palette.black
        };
        let columns = self.viewport.size.width.saturating_sub(TEXT_PAD * 2) as usize
            / Font::Body.metrics().character_size.width as usize;
        let text = fit_text(&status.text, super::model::TEXT_CAPACITY);
        let split = if text.len() > columns && columns > 0 {
            text[..columns.min(text.len())]
                .rfind(' ')
                .filter(|index| *index > 0)
                .unwrap_or(columns)
        } else {
            text.len()
        };
        let (first, rest) = text.split_at(split);
        self.row(first, 12, Font::Body, foreground, background);
        self.row(rest.trim_start(), 12, Font::Body, foreground, background);
    }
}

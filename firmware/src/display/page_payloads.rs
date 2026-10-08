use core::fmt::Write;
use embassy_time::Instant;
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use firmware_exec::jobs::{JobHandle, Phase};
use heapless::String;

use super::page_common::TEXT_PAD;
use super::pages::{Page, PageContext, PageInput, PageRenderArgs, PageRenderData};
use crate::usb::hid::{self, HidResultStatus, Owner, SubmitError};

struct Preset {
    name: &'static str,
    layout: &'static str,
    launches_agent: bool,
    available: bool,
    program: &'static [u8],
}
include!(concat!(env!("OUT_DIR"), "/keyboard_presets.rs"));

pub struct PayloadsPageState {
    selected: usize,
    pending: Option<JobHandle>,
    launching_agent: bool,
    agent_deadline: Option<u64>,
    status: String<64>,
    keyboard: String<64>,
}
impl PayloadsPageState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            pending: None,
            launching_agent: false,
            agent_deadline: None,
            status: String::new(),
            keyboard: String::new(),
        }
    }
    fn set_status(&mut self, text: &str) {
        self.status.clear();
        let _ = self.status.push_str(text);
    }
    fn run_selected(&mut self) {
        if self.pending.is_some() || self.agent_deadline.is_some() {
            return;
        }
        let Some(preset) = PRESETS.get(self.selected) else {
            return;
        };
        if !preset.available {
            self.set_status("Agent binary missing from USB image");
            return;
        }
        if preset.launches_agent && crate::capabilities::host_agent_snapshot().present {
            self.set_status("Agent already connected");
            return;
        }
        match hid::submit_preset(preset.program) {
            Ok(handle) => {
                self.pending = Some(handle);
                self.launching_agent = preset.launches_agent;
                self.set_status("Keyboard sequence queued");
            }
            Err(SubmitError::Busy) => self.set_status("Keyboard busy; use Y to stop"),
            Err(SubmitError::UsbUnavailable) => self.set_status("USB keyboard unavailable"),
            Err(SubmitError::Invalid) => self.set_status("Invalid keyboard preset"),
        }
    }
}
impl Page for PayloadsPageState {
    fn on_reset(&mut self) {}
    fn handle_input(&mut self, input: &PageInput, _ctx: &PageContext) -> bool {
        if !input.actions_allowed() || input.menu_open {
            return false;
        }
        if input.a_released {
            self.selected = if self.selected == 0 {
                PRESETS.len().saturating_sub(1)
            } else {
                self.selected - 1
            };
        }
        if input.b_released {
            self.selected = (self.selected + 1) % PRESETS.len();
        }
        if input.x_released {
            self.run_selected();
        }
        input.a_released || input.b_released || input.x_released
    }
    fn on_tick(&mut self, _ctx: &PageContext) -> bool {
        let mut dirty = false;
        if let Some(result) = hid::LOCAL_RESULT.try_take()
            && self.pending == Some(result.handle)
        {
            self.pending = None;
            match result.status {
                HidResultStatus::Completed if self.launching_agent => {
                    self.set_status("Keys sent; waiting for agent handshake");
                    self.agent_deadline = Some(Instant::now().as_millis() + 15_000);
                }
                HidResultStatus::Completed => self.set_status("Keyboard sequence completed"),
                HidResultStatus::Cancelled => {
                    self.set_status("Cancelled; sequence will not resume")
                }
                HidResultStatus::Rejected => self.set_status("Keyboard sequence rejected"),
                HidResultStatus::UsbUnavailable => {
                    self.set_status("USB unavailable; sequence stopped")
                }
            }
            dirty = true;
        }
        if let Some(deadline) = self.agent_deadline {
            if crate::capabilities::host_agent_snapshot().present {
                self.set_status("Agent connected");
                self.agent_deadline = None;
                dirty = true;
            } else if Instant::now().as_millis() >= deadline {
                self.set_status("Keys sent; agent not detected");
                self.agent_deadline = None;
                dirty = true;
            }
        }
        let mut keyboard = String::new();
        if let Some(job) = hid::active_job() {
            let owner = match job.owner {
                Owner::Local => "local preset",
                Owner::Browser { .. } => "browser",
            };
            let phase = if job.cancelled {
                "stopping"
            } else {
                match job.phase {
                    Phase::Queued => "queued",
                    Phase::Running => "running",
                    Phase::Releasing => "releasing keys",
                }
            };
            let _ = write!(keyboard, "Keyboard: {owner}, {phase}");
        } else if hid::USB_READY.load(core::sync::atomic::Ordering::SeqCst) {
            let _ = keyboard.push_str("Keyboard: ready");
        } else {
            let _ = keyboard.push_str("Keyboard: USB unavailable");
        }
        if keyboard != self.keyboard {
            self.keyboard = keyboard;
            dirty = true;
        }
        dirty
    }
    fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        args: PageRenderArgs<'_, D>,
        _data: PageRenderData<'_>,
    ) {
        let PageRenderArgs {
            disp,
            line_x,
            mut y_pos,
            clear_w,
            content_width,
            bg_color,
            config,
            title_style,
            body_style,
            ..
        } = args;
        let _ = Rectangle::new(Point::new(line_x, y_pos - 12), Size::new(content_width, 18))
            .into_styled(PrimitiveStyle::with_fill(config.header_bg))
            .draw(disp);
        let _ = Text::new(
            "Payloads",
            Point::new(line_x + TEXT_PAD, y_pos),
            *title_style,
        )
        .draw(disp);
        y_pos += 22;
        for line in [
            "A/B: Select, X: Run, A+X: Menu",
            "Y: Stop active keyboard job",
            self.keyboard.as_str(),
        ] {
            draw_line(disp, line_x, y_pos, clear_w, bg_color, line, body_style);
            y_pos += 16;
        }
        y_pos += 8;
        for (idx, preset) in PRESETS.iter().enumerate() {
            let selected = idx == self.selected;
            let bg = if selected {
                config.palette.white
            } else {
                bg_color
            };
            let mut style = *body_style;
            style.text_color = Some(if selected {
                config.palette.black
            } else {
                config.palette.white
            });
            draw_line(disp, line_x, y_pos, clear_w, bg, preset.name, &style);
            y_pos += 16;
        }
        y_pos += 12;
        if let Some(preset) = PRESETS.get(self.selected) {
            let mut layout: String<48> = String::new();
            let _ = write!(layout, "Input layout: {}", preset.layout);
            draw_line(disp, line_x, y_pos, clear_w, bg_color, &layout, body_style);
            y_pos += 16;
            let detail = if !preset.available {
                "Unavailable: package host agent first"
            } else if preset.launches_agent {
                "Opens Terminal; copies and starts agent"
            } else {
                "Types Hello from Pico! in focused app"
            };
            draw_line(disp, line_x, y_pos, clear_w, bg_color, detail, body_style);
            y_pos += 24;
        }
        draw_line(
            disp,
            line_x,
            y_pos,
            clear_w,
            bg_color,
            &self.status,
            body_style,
        );
    }
}
fn draw_line<D: DrawTarget<Color = Rgb565>>(
    disp: &mut D,
    x: i32,
    y: i32,
    width: u32,
    bg: Rgb565,
    text: &str,
    style: &embedded_graphics::mono_font::MonoTextStyle<'_, Rgb565>,
) {
    let _ = Rectangle::new(Point::new(x, y - 11), Size::new(width, 16))
        .into_styled(PrimitiveStyle::with_fill(bg))
        .draw(disp);
    let _ = Text::new(text, Point::new(x + TEXT_PAD, y), *style).draw(disp);
}

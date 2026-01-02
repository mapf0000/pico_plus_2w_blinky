use core::fmt::Write as _;
use core::sync::atomic::Ordering;

use embedded_graphics::mono_font::{MonoTextStyle, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use heapless::{String, Vec};

use crate::usb::hid::{HID_CHAN, HidCommand, MAX_BYTECODE, USB_READY};

use super::page_common::{update_line, TEXT_PAD};
use super::pages::{Page, PageContext, PageInput, PageRenderArgs, PageRenderData};
use super::{DisplayConfig, DisplayPalette};

const EXEC_TICKS: u8 = 6;
const GREEN_HOLD_TICKS: u8 = 6;

#[derive(Clone, Copy)]
enum StatusPhase {
    Idle,
    Executing { ticks: u8 },
    GreenHold { ticks: u8 },
}

struct Payload {
    name: &'static str,
    detail: &'static str,
    program: &'static [u8],
    dsl: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/payloads_gen.rs"));

pub struct PayloadsPageState {
    pub selected: usize,
    pub status: String<64>,
    pub prev_status: String<64>,
    pub status_bg: Rgb565,
    pub prev_status_bg: Rgb565,
    status_phase: StatusPhase,
    last_payload: Option<&'static str>,
    pending_payload: Option<usize>,
    pub details_open: bool,
    detail_idx: usize,
    detail_prev_lines: Vec<String<64>, 16>,
}

impl PayloadsPageState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            status: String::new(),
            prev_status: String::new(),
            status_bg: Rgb565::BLACK,
            prev_status_bg: Rgb565::BLACK,
            status_phase: StatusPhase::Idle,
            last_payload: None,
            pending_payload: None,
            details_open: false,
            detail_idx: 0,
            detail_prev_lines: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.prev_status.clear();
        self.status_bg = Rgb565::BLACK;
        self.prev_status_bg = Rgb565::BLACK;
        self.status_phase = StatusPhase::Idle;
        self.last_payload = None;
        self.pending_payload = None;
        self.details_open = false;
        self.detail_prev_lines.clear();
    }

    pub fn select_prev(&mut self) -> bool {
        if PAYLOADS.is_empty() {
            return false;
        }
        let old = self.selected;
        self.selected = if self.selected == 0 {
            PAYLOADS.len() - 1
        } else {
            self.selected.saturating_sub(1)
        };
        self.selected != old
    }

    pub fn select_next(&mut self) -> bool {
        if PAYLOADS.is_empty() {
            return false;
        }
        let old = self.selected;
        self.selected = (self.selected + 1) % PAYLOADS.len();
        self.selected != old
    }

    pub fn run_selected(&mut self, palette: &DisplayPalette, idle_bg: Rgb565) -> bool {
        if PAYLOADS.is_empty() {
            return false;
        }
        if !USB_READY.load(Ordering::SeqCst) {
            self.pending_payload = Some(self.selected);
            self.set_status("Waiting for USB, queued", idle_bg);
            return true;
        }
        self.start_payload(self.selected, palette, idle_bg)
    }

    fn set_status(&mut self, text: &str, bg: Rgb565) {
        self.status.clear();
        let _ = self.status.push_str(text);
        self.status_bg = bg;
        self.status_phase = StatusPhase::Idle;
    }

    pub fn toggle_details(&mut self) -> bool {
        if PAYLOADS.is_empty() {
            return false;
        }
        if self.details_open && self.detail_idx == self.selected {
            self.details_open = false;
        } else {
            self.details_open = true;
            self.detail_idx = self.selected;
            self.detail_prev_lines.clear();
        }
        true
    }

    fn start_payload(
        &mut self,
        idx: usize,
        palette: &DisplayPalette,
        idle_bg: Rgb565,
    ) -> bool {
        let payload = match PAYLOADS.get(idx) {
            Some(p) => p,
            None => return false,
        };
        let mut program = Vec::<u8, { MAX_BYTECODE }>::new();
        if program.extend_from_slice(payload.program).is_err() {
            self.set_status("Payload too large", idle_bg);
            return true;
        }
        match HID_CHAN.try_send(HidCommand::RunBytecode { program }) {
            Ok(()) => {
                self.status.clear();
                let _ = write!(self.status, "Executing: {}", payload.name);
                self.status_bg = palette.yellow;
                self.status_phase = StatusPhase::Executing { ticks: EXEC_TICKS };
                self.last_payload = Some(payload.name);
                log::info!("payload executing: {}", payload.name);
            }
            Err(_) => {
                self.set_status("Keyboard busy", idle_bg);
                log::warn!("payload queue busy; drop {}", payload.name);
            }
        }
        true
    }

    pub fn tick(&mut self, palette: &DisplayPalette, idle_bg: Rgb565) -> bool {
        if let Some(idx) = self.pending_payload {
            if USB_READY.load(Ordering::SeqCst) {
                self.pending_payload = None;
                return self.start_payload(idx, palette, idle_bg);
            }
        }
        match self.status_phase {
            StatusPhase::Idle => false,
            StatusPhase::Executing { ref mut ticks } => {
                if *ticks > 0 {
                    *ticks -= 1;
                }
                if *ticks == 0 {
                    if let Some(name) = self.last_payload {
                        self.status.clear();
                        let _ = write!(self.status, "Executed: {}", name);
                    }
                    self.status_bg = palette.green;
                    self.status_phase = StatusPhase::GreenHold {
                        ticks: GREEN_HOLD_TICKS,
                    };
                    return true;
                }
                false
            }
            StatusPhase::GreenHold { ref mut ticks } => {
                if *ticks > 0 {
                    *ticks -= 1;
                }
                if *ticks == 0 {
                    self.status_phase = StatusPhase::Idle;
                    self.status_bg = idle_bg;
                    return true;
                }
                false
            }
        }
    }
}

impl Page for PayloadsPageState {
    fn on_reset(&mut self) {
        PayloadsPageState::reset(self);
    }

    fn handle_input(&mut self, input: &PageInput, ctx: &PageContext) -> bool {
        let mut dirty = false;
        if input.y_released && !input.menu_open && input.actions_allowed() {
            dirty |= self.toggle_details();
        }
        if input.actions_allowed() && !self.details_open {
            if !input.menu_open && input.a_released {
                dirty |= self.select_prev();
            }
            if !input.menu_open && input.b_released {
                dirty |= self.select_next();
            }
            if input.x_released {
                dirty |= self.run_selected(ctx.palette, ctx.idle_bg);
            }
        }
        dirty
    }

    fn on_tick(&mut self, ctx: &PageContext) -> bool {
        self.tick(ctx.palette, ctx.idle_bg)
    }

    fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        args: PageRenderArgs<'_, D>,
        _data: PageRenderData<'_>,
    ) {
        render(
            args.disp,
            args.line_x,
            args.y_pos,
            args.clear_w,
            args.content_width,
            args.content_height,
            args.bg_color,
            args.config,
            args.title_style,
            args.body_style,
            self,
        );
    }
}

fn render_details(
    disp: &mut impl DrawTarget<Color = Rgb565>,
    line_x: i32,
    mut y_pos: i32,
    clear_w: u32,
    bg_color: Rgb565,
    content_height: u32,
    body_style: &MonoTextStyle<Rgb565>,
    state: &mut PayloadsPageState,
) {
    // Clear detail area first.
    let header_gap = 4u32;
    let detail_height = content_height.saturating_sub((y_pos as u32).saturating_sub(header_gap));
    let _ = Rectangle::new(Point::new(line_x, y_pos - 11), Size::new(clear_w, detail_height))
        .into_styled(PrimitiveStyle::with_fill(bg_color))
        .draw(disp);

    let mut lines: Vec<String<64>, 16> = Vec::new();
    let payload = match PAYLOADS.get(state.detail_idx) {
        Some(p) => p,
        None => return,
    };

    let mut name_line: String<64> = String::new();
    let _ = write!(name_line, "Name: {}", payload.name);
    let mut detail_line: String<64> = String::new();
    let _ = write!(detail_line, "Detail: {}", payload.detail);
    let mut len_line: String<64> = String::new();
    let _ = write!(len_line, "Length: {} bytes", payload.program.len());

    let _ = lines.push(name_line);
    let _ = lines.push(detail_line);
    let _ = lines.push(len_line);
    let mut dsl_label: String<64> = String::new();
    let _ = dsl_label.push_str("DSL:");
    let _ = lines.push(dsl_label);
    for dl in payload.dsl.lines() {
        if lines.is_full() {
            break;
        }
        let mut s: String<64> = String::new();
        let _ = s.push_str(dl);
        let _ = lines.push(s);
    }
    if !lines.is_full() {
        let mut hint: String<64> = String::new();
        let _ = hint.push_str("Y to close");
        let _ = lines.push(hint);
    }

    if state.detail_prev_lines.len() > lines.len() {
        state.detail_prev_lines.truncate(lines.len());
    }
    while state.detail_prev_lines.len() < lines.len() {
        state.detail_prev_lines.push(String::new()).ok();
    }

    let line_h: i32 = 14;
    for (idx, line) in lines.iter().enumerate() {
        update_line(
            disp,
            line_x,
            y_pos,
            clear_w,
            line_h as u32,
            bg_color,
            line.as_str(),
            &mut state.detail_prev_lines[idx],
            body_style,
        );
        y_pos += line_h;
    }
}

pub fn render(
    disp: &mut impl DrawTarget<Color = Rgb565>,
    line_x: i32,
    mut y_pos: i32,
    clear_w: u32,
    content_width: u32,
    content_height: u32,
    bg_color: Rgb565,
    config: &DisplayConfig,
    title_style: &MonoTextStyle<Rgb565>,
    body_style: &MonoTextStyle<Rgb565>,
    state: &mut PayloadsPageState,
) {
    let palette = config.palette;
    let header_height = 18;
    let _ = Rectangle::new(
        Point::new(line_x, y_pos - 12),
        Size::new(content_width, header_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(config.header_bg))
    .draw(disp);
    let title = if state.details_open { "Payload Details" } else { "Payloads" };
    let _ = Text::new(title, Point::new(line_x + TEXT_PAD, y_pos), *title_style).draw(disp);

    y_pos += header_height;

    if !state.details_open && !state.detail_prev_lines.is_empty() {
        // Clear everything below the header to remove detail text remnants.
        let clear_start_y = y_pos - 11;
        let clear_h = content_height.saturating_sub(clear_start_y as u32);
        let _ = Rectangle::new(Point::new(line_x, clear_start_y), Size::new(clear_w, clear_h))
            .into_styled(PrimitiveStyle::with_fill(bg_color))
            .draw(disp);
        state.detail_prev_lines.clear();
        state.prev_status.clear();
        state.prev_status_bg = Rgb565::BLACK;
    }

    if state.details_open {
        render_details(
            disp,
            line_x,
            y_pos,
            clear_w,
            bg_color,
            content_height,
            body_style,
            state,
        );
        return;
    }

    let line_h: i32 = 16;
    let status_text = if state.status.is_empty() {
        "A/B: Up/Down, X: Run, A+X: Menu"
    } else {
        state.status.as_str()
    };
    if state.prev_status_bg != state.status_bg {
        state.prev_status.clear();
    }
    let status_text_color = if state.status_bg == palette.black {
        palette.white
    } else {
        palette.black
    };
    let status_style = MonoTextStyleBuilder::new()
        .font(body_style.font)
        .text_color(status_text_color)
        .background_color(state.status_bg)
        .build();
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        state.status_bg,
        status_text,
        &mut state.prev_status,
        &status_style,
    );
    state.prev_status_bg = state.status_bg;
    let status_block_bottom = (y_pos - 11) + line_h;
    let divider_y = status_block_bottom;
    let divider_color = if state.status_bg != palette.black {
        state.status_bg
    } else {
        palette.white
    };
    let _ = Rectangle::new(Point::new(line_x, divider_y), Size::new(clear_w, 2))
        .into_styled(PrimitiveStyle::with_fill(divider_color))
        .draw(disp);
    let gap_after_status: i32 = 18;
    y_pos = status_block_bottom + gap_after_status;

    let row_h: i32 = 16;
    for (idx, payload) in PAYLOADS.iter().enumerate() {
        let row_y = y_pos + (idx as i32 * row_h);
        let selected = idx == state.selected;
        let row_bg = if selected { palette.white } else { bg_color };
        let row_fg = if selected { palette.black } else { palette.white };

        let _ = Rectangle::new(Point::new(line_x, row_y - 11), Size::new(clear_w, row_h as u32))
            .into_styled(PrimitiveStyle::with_fill(row_bg))
            .draw(disp);

        let row_style = MonoTextStyleBuilder::new()
            .font(body_style.font)
            .text_color(row_fg)
            .background_color(row_bg)
            .build();
        let _ = Text::new(payload.name, Point::new(line_x + TEXT_PAD, row_y), row_style).draw(disp);
    }
}

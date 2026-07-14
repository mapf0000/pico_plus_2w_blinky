use core::fmt::Write as _;
use core::sync::atomic::Ordering;

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use heapless::String;

use crate::http::routes::ws;
use crate::usb::ctrl::{
    self, CTRL_READY, TransferRelayMode, TransferViewSnapshot, TransferViewState,
};

use super::DisplayPalette;
use super::page_common::{TEXT_PAD, update_line};
use super::pages::{Page, PageContext, PageInput, PageRenderArgs, PageRenderData};

const ACTION_START_TRANSFER: usize = 0;
const ACTION_TOGGLE_MODE: usize = 1;
const ACTION_COUNT: usize = 2;

pub struct TransferPageState {
    pub selected: usize,
    pub status: String<64>,
    pub prev_status: String<64>,
    pub status_bg: Rgb565,
    pub prev_status_bg: Rgb565,
    prev_usb_line: String<64>,
    prev_ws_line: String<64>,
    prev_mode_line: String<64>,
    prev_source_line: String<64>,
    prev_state_line: String<64>,
    prev_id_line: String<64>,
    prev_bytes_line: String<64>,
    prev_chunks_line: String<64>,
}

impl TransferPageState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            status: String::new(),
            prev_status: String::new(),
            status_bg: Rgb565::BLACK,
            prev_status_bg: Rgb565::BLACK,
            prev_usb_line: String::new(),
            prev_ws_line: String::new(),
            prev_mode_line: String::new(),
            prev_source_line: String::new(),
            prev_state_line: String::new(),
            prev_id_line: String::new(),
            prev_bytes_line: String::new(),
            prev_chunks_line: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.prev_status.clear();
        self.status_bg = Rgb565::BLACK;
        self.prev_status_bg = Rgb565::BLACK;
        self.prev_usb_line.clear();
        self.prev_ws_line.clear();
        self.prev_mode_line.clear();
        self.prev_source_line.clear();
        self.prev_state_line.clear();
        self.prev_id_line.clear();
        self.prev_bytes_line.clear();
        self.prev_chunks_line.clear();
    }

    fn select_prev(&mut self) -> bool {
        let old = self.selected;
        self.selected = if self.selected == 0 {
            ACTION_COUNT - 1
        } else {
            self.selected.saturating_sub(1)
        };
        self.selected != old
    }

    fn select_next(&mut self) -> bool {
        let old = self.selected;
        self.selected = (self.selected + 1) % ACTION_COUNT;
        self.selected != old
    }

    fn set_status(&mut self, text: &str, bg: Rgb565) {
        self.status.clear();
        let _ = self.status.push_str(text);
        self.status_bg = bg;
    }

    fn run_selected(&mut self, palette: &DisplayPalette) -> bool {
        match self.selected {
            ACTION_START_TRANSFER => {
                self.set_status("Pair and start in Web UI", palette.yellow);
                true
            }
            ACTION_TOGGLE_MODE => {
                ctrl::set_transfer_relay_mode(TransferRelayMode::RelayToBrowser);
                self.set_status("Encrypted relay is required", palette.green);
                true
            }
            _ => false,
        }
    }
}

impl Page for TransferPageState {
    fn on_reset(&mut self) {
        TransferPageState::reset(self);
    }

    fn handle_input(&mut self, input: &PageInput, ctx: &PageContext) -> bool {
        let mut dirty = false;
        if input.actions_allowed() && !input.menu_open {
            if input.a_released {
                dirty |= self.select_prev();
            }
            if input.b_released {
                dirty |= self.select_next();
            }
            if input.x_released {
                dirty |= self.run_selected(ctx.palette);
            }
        }
        dirty
    }

    fn on_tick(&mut self, _ctx: &PageContext) -> bool {
        false
    }

    fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        args: PageRenderArgs<'_, D>,
        _data: PageRenderData<'_>,
    ) {
        render(args, self);
    }
}

fn mode_name(mode: TransferRelayMode) -> &'static str {
    match mode {
        TransferRelayMode::RelayToBrowser => "relay->browser",
        TransferRelayMode::SimulationDrop => "simulation(drop)",
    }
}

fn state_name(state: TransferViewState) -> &'static str {
    match state {
        TransferViewState::Idle => "idle",
        TransferViewState::Open => "open",
        TransferViewState::Progress => "progress",
        TransferViewState::Finished => "finished",
        TransferViewState::Failed => "failed",
        TransferViewState::Aborted => "aborted",
    }
}

struct ActionRowContext<'a> {
    line_x: i32,
    clear_w: u32,
    bg_color: Rgb565,
    body_style: &'a MonoTextStyle<'a, Rgb565>,
    palette: &'a DisplayPalette,
}

fn draw_action_row(
    disp: &mut impl DrawTarget<Color = Rgb565>,
    context: &ActionRowContext<'_>,
    row_y: i32,
    label: &str,
    selected: bool,
) {
    let row_h: i32 = 16;
    let row_bg = if selected {
        context.palette.white
    } else {
        context.bg_color
    };
    let row_fg = if selected {
        context.palette.black
    } else {
        context.palette.white
    };

    let _ = Rectangle::new(
        Point::new(context.line_x, row_y - 11),
        Size::new(context.clear_w, row_h as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(row_bg))
    .draw(disp);

    let row_style = MonoTextStyleBuilder::new()
        .font(context.body_style.font)
        .text_color(row_fg)
        .background_color(row_bg)
        .build();
    let _ = Text::new(
        label,
        Point::new(context.line_x + TEXT_PAD, row_y),
        row_style,
    )
    .draw(disp);
}

fn render<D: DrawTarget<Color = Rgb565>>(
    args: PageRenderArgs<'_, D>,
    state: &mut TransferPageState,
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
    let palette = config.palette;
    let snapshot: TransferViewSnapshot = ctrl::transfer_view_snapshot();
    let usb_ready = CTRL_READY.load(Ordering::Acquire);
    let ws_connected = ws::has_active_client();

    let header_height = 18;
    let _ = Rectangle::new(
        Point::new(line_x, y_pos - 12),
        Size::new(content_width, header_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(config.header_bg))
    .draw(disp);
    let _ = Text::new(
        "Transfer",
        Point::new(line_x + TEXT_PAD, y_pos),
        *title_style,
    )
    .draw(disp);
    y_pos += header_height;

    let line_h: i32 = 14;
    let status_text = if state.status.is_empty() {
        "A/B: Select, X: Act, A+X: Menu"
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
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        state.status_bg,
        status_text,
        &mut state.prev_status,
        &status_style,
    );
    state.prev_status_bg = state.status_bg;
    y_pos += line_h + 4;

    let mut usb_line: String<64> = String::new();
    let _ = write!(
        usb_line,
        "USB ctrl: {}",
        if usb_ready { "ready" } else { "waiting" }
    );
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        usb_line.as_str(),
        &mut state.prev_usb_line,
        body_style,
    );
    y_pos += line_h;

    let mut ws_line: String<64> = String::new();
    let _ = write!(
        ws_line,
        "Browser WS: {}",
        if ws_connected { "connected" } else { "none" }
    );
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        ws_line.as_str(),
        &mut state.prev_ws_line,
        body_style,
    );
    y_pos += line_h;

    let mut mode_line: String<64> = String::new();
    let _ = write!(mode_line, "Mode: {}", mode_name(snapshot.mode));
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        mode_line.as_str(),
        &mut state.prev_mode_line,
        body_style,
    );
    y_pos += line_h;

    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        "Source: host default path",
        &mut state.prev_source_line,
        body_style,
    );
    y_pos += line_h;

    let mut transfer_state_line: String<64> = String::new();
    let _ = write!(transfer_state_line, "State: {}", state_name(snapshot.state));
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        transfer_state_line.as_str(),
        &mut state.prev_state_line,
        body_style,
    );
    y_pos += line_h;

    let mut id_line: String<64> = String::new();
    if snapshot.transfer_id == 0 {
        let _ = id_line.push_str("Transfer id: -");
    } else {
        let _ = write!(id_line, "Transfer id: {}", snapshot.transfer_id);
    }
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        id_line.as_str(),
        &mut state.prev_id_line,
        body_style,
    );
    y_pos += line_h;

    let mut bytes_line: String<64> = String::new();
    let _ = write!(
        bytes_line,
        "Bytes: {} / {}",
        snapshot.received_size, snapshot.total_size
    );
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        bytes_line.as_str(),
        &mut state.prev_bytes_line,
        body_style,
    );
    y_pos += line_h;

    let mut chunks_line: String<64> = String::new();
    let _ = write!(
        chunks_line,
        "Chunks: {} / {}",
        snapshot.finished_chunks, snapshot.chunk_count
    );
    update_line(
        disp,
        Point::new(line_x, y_pos),
        Size::new(clear_w, line_h as u32),
        bg_color,
        chunks_line.as_str(),
        &mut state.prev_chunks_line,
        body_style,
    );
    y_pos += line_h + 8;

    let start_selected = state.selected == ACTION_START_TRANSFER;
    let action_context = ActionRowContext {
        line_x,
        clear_w,
        bg_color,
        body_style,
        palette: &palette,
    };
    draw_action_row(
        disp,
        &action_context,
        y_pos,
        "Start in paired Web UI",
        start_selected,
    );
    y_pos += 16;

    let mode_selected = state.selected == ACTION_TOGGLE_MODE;
    let mode_action_label = "Mode: Encrypted relay only";
    draw_action_row(
        disp,
        &action_context,
        y_pos,
        mode_action_label,
        mode_selected,
    );
}

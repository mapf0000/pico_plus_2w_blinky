use core::fmt::Write as _;

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use heapless::String;

use crate::usb::ctrl::{CTRL_CHAN, CTRL_READY, CtrlCommand};

use super::page_common::{TEXT_PAD, update_line};
use super::pages::{Page, PageContext, PageInput, PageRenderArgs, PageRenderData};
use super::{DisplayConfig, DisplayPalette};

#[derive(Clone, Copy)]
enum DaemonActionKind {
    RequestStatus,
    Execute { command: &'static str },
    RequestDbCredentials { prompt: &'static str },
}

#[derive(Clone, Copy)]
struct DaemonAction {
    name: &'static str,
    detail: &'static str,
    kind: DaemonActionKind,
}

const ACTIONS: &[DaemonAction] = &[
    DaemonAction {
        name: "Agent status",
        detail: "Request agent status",
        kind: DaemonActionKind::RequestStatus,
    },
    DaemonAction {
        name: "Run whoami",
        detail: "Execute `whoami` on host",
        kind: DaemonActionKind::Execute { command: "whoami" },
    },
    DaemonAction {
        name: "Run hostname",
        detail: "Execute `hostname` on host",
        kind: DaemonActionKind::Execute {
            command: "hostname",
        },
    },
    DaemonAction {
        name: "Open google.com",
        detail: "Open https://google.com in default browser",
        kind: DaemonActionKind::Execute {
            command: "open https://google.com",
        },
    },
    DaemonAction {
        name: "DB credentials",
        detail: "Request database credentials from host",
        kind: DaemonActionKind::RequestDbCredentials {
            prompt: "Database credentials",
        },
    },
];

pub struct DaemonPageState {
    pub selected: usize,
    pub status: String<64>,
    pub prev_status: String<64>,
    pub status_bg: Rgb565,
    pub prev_status_bg: Rgb565,
}

impl DaemonPageState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            status: String::new(),
            prev_status: String::new(),
            status_bg: Rgb565::BLACK,
            prev_status_bg: Rgb565::BLACK,
        }
    }

    pub fn reset(&mut self) {
        self.prev_status.clear();
        self.status_bg = Rgb565::BLACK;
        self.prev_status_bg = Rgb565::BLACK;
    }

    pub fn select_prev(&mut self) -> bool {
        if ACTIONS.is_empty() {
            return false;
        }
        let old = self.selected;
        self.selected = if self.selected == 0 {
            ACTIONS.len() - 1
        } else {
            self.selected.saturating_sub(1)
        };
        self.selected != old
    }

    pub fn select_next(&mut self) -> bool {
        if ACTIONS.is_empty() {
            return false;
        }
        let old = self.selected;
        self.selected = (self.selected + 1) % ACTIONS.len();
        self.selected != old
    }

    pub fn run_selected(&mut self, palette: &DisplayPalette, _idle_bg: Rgb565) -> bool {
        let action = match ACTIONS.get(self.selected) {
            Some(action) => action,
            None => return false,
        };
        if !CTRL_READY.load(core::sync::atomic::Ordering::SeqCst) {
            self.status.clear();
            let _ = write!(self.status, "Waiting for USB: {}", action.name);
            self.status_bg = palette.yellow;
            return true;
        }

        let cmd = match action.kind {
            DaemonActionKind::RequestStatus => CtrlCommand::RequestStatus,
            DaemonActionKind::Execute { command } => CtrlCommand::Execute { command },
            DaemonActionKind::RequestDbCredentials { prompt } => {
                CtrlCommand::RequestDbCredentials { prompt }
            }
        };

        let send_result = CTRL_CHAN.try_send(cmd);
        self.status.clear();
        match send_result {
            Ok(()) => {
                let _ = write!(self.status, "Sent: {}", action.name);
                self.status_bg = palette.green;
            }
            Err(_) => {
                let _ = write!(self.status, "Queue busy: {}", action.name);
                self.status_bg = palette.yellow;
            }
        }
        log::info!(
            "host agent action sent: {} (detail={})",
            action.name,
            action.detail,
        );
        true
    }
}

impl Page for DaemonPageState {
    fn on_reset(&mut self) {
        DaemonPageState::reset(self);
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
                dirty |= self.run_selected(ctx.palette, ctx.idle_bg);
            }
        }
        dirty
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
    state: &mut DaemonPageState,
) {
    let palette = config.palette;
    let header_height = 18;
    let _ = Rectangle::new(
        Point::new(line_x, y_pos - 12),
        Size::new(content_width, header_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(config.header_bg))
    .draw(disp);
    let _ = Text::new(
        "Host Agent",
        Point::new(line_x + TEXT_PAD, y_pos),
        *title_style,
    )
    .draw(disp);
    y_pos += header_height;

    let line_h: i32 = 16;
    let status_text = if state.status.is_empty() {
        "A/B: Up/Down, X: Send, A+X: Menu"
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
    let max_rows = (content_height.saturating_sub(y_pos as u32) / row_h as u32) as usize;
    let visible_actions = ACTIONS.len().min(max_rows);
    for idx in 0..visible_actions {
        let action = &ACTIONS[idx];
        let row_y = y_pos + (idx as i32 * row_h);
        let selected = idx == state.selected;
        let row_bg = if selected { palette.white } else { bg_color };
        let row_fg = if selected {
            palette.black
        } else {
            palette.white
        };

        let _ = Rectangle::new(
            Point::new(line_x, row_y - 11),
            Size::new(clear_w, row_h as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(row_bg))
        .draw(disp);

        let row_style = MonoTextStyleBuilder::new()
            .font(body_style.font)
            .text_color(row_fg)
            .background_color(row_bg)
            .build();
        let _ = Text::new(action.name, Point::new(line_x + TEXT_PAD, row_y), row_style).draw(disp);
    }
}

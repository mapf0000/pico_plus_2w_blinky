use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use heapless::{String, Vec};

use crate::log_buffer;

use super::page_common::{update_line, TEXT_PAD};
use super::pages::{Page, PageContext, PageInput, PageRenderArgs, PageRenderData};
use super::DisplayConfig;

pub struct LogsPageState {
    pub prev_lines: Vec<String<{ log_buffer::LOG_LINE_MAX }>, { log_buffer::LOG_CAPACITY }>,
    pub last_gen: u32,
}

impl LogsPageState {
    pub fn new() -> Self {
        Self {
            prev_lines: Vec::new(),
            last_gen: 0,
        }
    }

    pub fn reset(&mut self) {
        self.prev_lines.clear();
    }
}

impl Page for LogsPageState {
    fn on_reset(&mut self) {
        LogsPageState::reset(self);
    }

    fn handle_input(&mut self, _input: &PageInput, _ctx: &PageContext) -> bool {
        false
    }

    fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        args: PageRenderArgs<'_, D>,
        data: PageRenderData<'_>,
    ) {
        let log_gen = match data {
            PageRenderData::Logs { log_gen } => log_gen,
            _ => {
                debug_assert!(false, "logs page render missing generation");
                return;
            }
        };

        render(
            args.disp,
            args.line_x,
            args.y_pos,
            args.clear_w,
            args.content_width,
            args.content_height,
            args.config,
            log_gen,
            args.bg_color,
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
    config: &DisplayConfig,
    log_gen: u32,
    bg_color: Rgb565,
    title_style: &MonoTextStyle<Rgb565>,
    body_style: &MonoTextStyle<Rgb565>,
    state: &mut LogsPageState,
) {
    let mut lines: Vec<
        String<{ log_buffer::LOG_LINE_MAX }>,
        { log_buffer::LOG_CAPACITY },
    > = Vec::new();
    log_buffer::snapshot(&mut lines);

    let header_height = 18;
    let _ = Rectangle::new(
        Point::new(line_x, y_pos - 12),
        Size::new(content_width, header_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(config.header_bg))
    .draw(disp);
    let _ = Text::new("Logs", Point::new(line_x + TEXT_PAD, y_pos), *title_style).draw(disp);
    y_pos += 18;

    let line_h: i32 = 12;
    let available_height = content_height.saturating_sub(y_pos as u32);
    let max_lines = (available_height / (line_h as u32))
        .min(log_buffer::LOG_CAPACITY as u32) as usize;
    let start_idx = lines.len().saturating_sub(max_lines);
    let display_slice = &lines[start_idx..];

    let old_len = state.prev_lines.len();
    if old_len > display_slice.len() {
        state.prev_lines.truncate(display_slice.len());
    }
    while state.prev_lines.len() < display_slice.len() {
        state.prev_lines.push(String::new()).ok();
    }

    for (idx, line) in display_slice.iter().enumerate() {
        update_line(
            disp,
            line_x,
            y_pos,
            clear_w,
            line_h as u32,
            bg_color,
            line.as_str(),
            &mut state.prev_lines[idx],
            body_style,
        );
        y_pos += line_h;
    }

    if old_len > display_slice.len() {
        let clear_h = ((old_len - display_slice.len()) as i32 * line_h).max(0) as u32;
        if clear_h > 0 {
            let _ = Rectangle::new(Point::new(line_x, y_pos), Size::new(clear_w, clear_h))
                .into_styled(PrimitiveStyle::with_fill(bg_color))
                .draw(disp);
        }
    }

    state.last_gen = log_gen;
}

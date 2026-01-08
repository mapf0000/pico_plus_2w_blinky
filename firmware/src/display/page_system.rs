use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use heapless::String;

use super::page_common::{build_text_line, update_line, TEXT_PAD};
use super::pages::{Page, PageContext, PageInput, PageRenderArgs, PageRenderData};
use super::DisplayConfig;

pub struct SystemMetrics<'a> {
    pub uptime: &'a str,
    pub cpu: &'a str,
    pub temp: &'a str,
    pub heap: &'a str,
    pub stack: &'a str,
    pub psram: &'a str,
    pub flash: &'a str,
}

pub struct SystemPageState {
    prev_ap_line: String<32>,
    prev_uptime_line: String<32>,
    prev_cpu_line: String<32>,
    prev_temp_line: String<32>,
    prev_heap_line: String<32>,
    prev_stack_line: String<32>,
    prev_psram_line: String<32>,
    prev_flash_line: String<48>,
}

impl SystemPageState {
    pub fn new() -> Self {
        Self {
            prev_ap_line: String::new(),
            prev_uptime_line: String::new(),
            prev_cpu_line: String::new(),
            prev_temp_line: String::new(),
            prev_heap_line: String::new(),
            prev_stack_line: String::new(),
            prev_psram_line: String::new(),
            prev_flash_line: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.prev_ap_line.clear();
        self.prev_uptime_line.clear();
        self.prev_cpu_line.clear();
        self.prev_temp_line.clear();
        self.prev_heap_line.clear();
        self.prev_stack_line.clear();
        self.prev_psram_line.clear();
        self.prev_flash_line.clear();
    }
}

impl Page for SystemPageState {
    fn on_reset(&mut self) {
        SystemPageState::reset(self);
    }

    fn handle_input(&mut self, _input: &PageInput, _ctx: &PageContext) -> bool {
        false
    }

    fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        args: PageRenderArgs<'_, D>,
        data: PageRenderData<'_>,
    ) {
        let (ap_ssid, metrics) = match data {
            PageRenderData::System { ap_ssid, metrics } => (ap_ssid, metrics),
            _ => {
                debug_assert!(false, "system page render missing metrics");
                return;
            }
        };

        render(
            args.disp,
            args.line_x,
            args.y_pos,
            args.content_width,
            args.clear_w,
            args.bg_color,
            args.config,
            ap_ssid,
            metrics,
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
    content_width: u32,
    clear_w: u32,
    bg_color: Rgb565,
    config: &DisplayConfig,
    ap_ssid: &str,
    metrics: SystemMetrics<'_>,
    title_style: &MonoTextStyle<Rgb565>,
    body_style: &MonoTextStyle<Rgb565>,
    state: &mut SystemPageState,
) {
    let header_height = 18;
    let _ = Rectangle::new(
        Point::new(line_x, y_pos - 12),
        Size::new(content_width, header_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(config.header_bg))
    .draw(disp);
    let _ = Text::new(
        "System / Pico Plus 2",
        Point::new(line_x + TEXT_PAD, y_pos),
        *title_style,
    )
        .draw(disp);

    y_pos += 18;
    let ap_line: String<32> = build_text_line("AP: ", ap_ssid);

    let line_h: i32 = 14;

    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        ap_line.as_str(),
        &mut state.prev_ap_line,
        &body_style,
    );
    y_pos += line_h;

    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.uptime,
        &mut state.prev_uptime_line,
        &body_style,
    );
    y_pos += line_h;
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.cpu,
        &mut state.prev_cpu_line,
        &body_style,
    );
    y_pos += line_h;
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.temp,
        &mut state.prev_temp_line,
        &body_style,
    );
    y_pos += line_h;
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.heap,
        &mut state.prev_heap_line,
        &body_style,
    );
    y_pos += line_h;
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.stack,
        &mut state.prev_stack_line,
        &body_style,
    );
    y_pos += line_h;
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.psram,
        &mut state.prev_psram_line,
        &body_style,
    );
    y_pos += line_h;
    update_line(
        disp,
        line_x,
        y_pos,
        clear_w,
        line_h as u32,
        bg_color,
        metrics.flash,
        &mut state.prev_flash_line,
        &body_style,
    );
}

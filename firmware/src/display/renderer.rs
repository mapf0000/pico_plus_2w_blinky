use embedded_graphics::mono_font::ascii::{FONT_6X9, FONT_8X13};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;

use super::pages::{MenuItem, PageId, PageRegistry, PageRenderArgs, PageRenderData};
use super::ui::UiState;
use super::{DisplayConfig, DisplayPalette};

#[derive(Clone, Copy)]
struct Layout {
    menu_rect: Rectangle,
    menu_clear_rect: Rectangle,
    divider_rect: Option<Rectangle>,
    content_rect: Rectangle,
    content_width: u32,
}

impl Layout {
    fn compute(screen: Size, cfg: &DisplayConfig, menu_open: bool) -> Self {
        let screen_w = screen.width;
        let screen_h = screen.height;
        let content_pad = if menu_open { cfg.content_padding } else { 0 };

        let menu_w = if menu_open { cfg.menu_width } else { 0 };
        let menu_rect = Rectangle::new(Point::new(0, 0), Size::new(menu_w, screen_h));
        let menu_clear_w = if menu_open {
            menu_w
                .saturating_add(cfg.gutter)
                .saturating_add(cfg.divider_width)
        } else {
            0
        };
        let menu_clear_rect = Rectangle::new(Point::new(0, 0), Size::new(menu_clear_w, screen_h));

        let divider_rect = if menu_open {
            let x = menu_w as i32 + cfg.gutter as i32;
            Some(Rectangle::new(
                Point::new(x, 0),
                Size::new(cfg.divider_width, screen_h),
            ))
        } else {
            None
        };

        let content_x = if menu_open {
            (menu_w + cfg.gutter + cfg.divider_width) as i32 + content_pad
        } else {
            content_pad
        };
        let content_w = screen_w
            .saturating_sub(content_x as u32)
            .saturating_sub(content_pad as u32);
        let content_rect =
            Rectangle::new(Point::new(content_x, 0), Size::new(content_w, screen_h));

        Self {
            menu_rect,
            menu_clear_rect,
            divider_rect,
            content_rect,
            content_width: screen_w.saturating_sub(content_x as u32),
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Dirty {
    menu: bool,
    divider: bool,
    content: bool,
    page: bool,
}

impl Dirty {
    fn any(self) -> bool {
        self.menu || self.divider || self.content || self.page
    }
}

pub struct RenderPlan {
    dirty: Dirty,
    layout: Layout,
    menu_changed: bool,
}

impl RenderPlan {
    pub fn should_draw(&self) -> bool {
        self.dirty.any()
    }

    pub fn needs_page_render(&self) -> bool {
        self.dirty.page || self.dirty.content
    }
}

struct RenderState {
    bg_drawn: bool,
    prev_page: PageId,
    prev_menu_selected: usize,
    prev_menu_open: bool,
    last_layout: Option<Layout>,
}

pub struct Renderer {
    config: DisplayConfig,
    palette: DisplayPalette,
    state: RenderState,
}

impl Renderer {
    pub fn new(config: DisplayConfig, initial_page: PageId) -> Self {
        let palette = config.palette;
        Self {
            config,
            palette,
            state: RenderState {
                bg_drawn: false,
                prev_page: initial_page,
                prev_menu_selected: usize::MAX,
                prev_menu_open: false,
                last_layout: None,
            },
        }
    }

    pub fn palette(&self) -> DisplayPalette {
        self.palette
    }

    pub fn plan<D: DrawTarget<Color = Rgb565>>(
        &self,
        disp: &D,
        ui: &UiState,
        page: PageId,
        page_dirty: bool,
        log_changed: bool,
    ) -> RenderPlan {
        let menu_changed = ui.menu_open && ui.menu_selected != self.state.prev_menu_selected;
        let menu_state_changed = ui.menu_open != self.state.prev_menu_open;
        let page_changed = page != self.state.prev_page;
        let mut dirty = Dirty::default();

        if !self.state.bg_drawn {
            dirty = Dirty {
                menu: true,
                divider: true,
                content: true,
                page: true,
            };
        }
        if menu_state_changed {
            dirty.menu = true;
            dirty.divider = true;
            dirty.content = true;
            dirty.page = true;
        }
        if page_changed {
            dirty.content = true;
            dirty.page = true;
        }
        if log_changed {
            dirty.page = true;
        }
        if page_dirty {
            dirty.page = true;
        }

        RenderPlan {
            dirty,
            layout: Layout::compute(disp.bounding_box().size, &self.config, ui.menu_open),
            menu_changed,
        }
    }

    pub fn render_with_plan<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        disp: &mut D,
        ui: &UiState,
        page: PageId,
        menu_items: &[MenuItem],
        registry: &mut PageRegistry,
        plan: RenderPlan,
        page_data: PageRenderData<'_>,
    ) {
        if !plan.dirty.any() {
            return;
        }

        let bg_color = self.palette.black;
        let menu_bg = self.palette.black;
        let layout = plan.layout;
        let screen_h = layout.content_rect.size.height.max(layout.menu_rect.size.height);
        let title_style = MonoTextStyleBuilder::new()
            .font(&FONT_8X13)
            .text_color(self.config.header_text)
            .background_color(self.config.header_bg)
            .build();
        let body_style = MonoTextStyleBuilder::new()
            .font(&FONT_6X9)
            .text_color(self.palette.white)
            .background_color(bg_color)
            .build();

        if !self.state.bg_drawn {
            let _ = disp.clear(bg_color);
        }

        if plan.dirty.menu {
            if let Some(prev_layout) = self.state.last_layout {
                if self.state.prev_menu_open {
                    let _ = prev_layout
                        .menu_clear_rect
                        .into_styled(PrimitiveStyle::with_fill(bg_color))
                        .draw(disp);
                }
            }
            if ui.menu_open {
                let _ = layout
                    .menu_clear_rect
                    .into_styled(PrimitiveStyle::with_fill(menu_bg))
                    .draw(disp);
                if let Some(divider) = layout.divider_rect {
                    let _ = divider
                        .into_styled(PrimitiveStyle::with_fill(self.palette.white))
                        .draw(disp);
                }
                let menu_base_y = 20;
                let menu_item_h: i32 = 18;
                for (idx, item) in menu_items.iter().enumerate() {
                    draw_menu_item(
                        disp,
                        layout.menu_rect.top_left.x,
                        layout.menu_rect.size.width,
                        menu_base_y,
                        menu_item_h,
                        idx,
                        item.label,
                        idx == ui.menu_selected,
                        menu_bg,
                        &self.palette,
                    );
                }
            }
        } else if plan.dirty.divider {
            if let Some(divider) = layout.divider_rect {
                let _ = divider
                    .into_styled(PrimitiveStyle::with_fill(self.palette.white))
                    .draw(disp);
            }
        } else if plan.menu_changed && ui.menu_open && self.state.bg_drawn {
            if self.state.prev_menu_selected != usize::MAX
                && self.state.prev_menu_selected < menu_items.len()
            {
                let menu_base_y = 20;
                let menu_item_h: i32 = 18;
                draw_menu_item(
                    disp,
                    layout.menu_rect.top_left.x,
                    layout.menu_rect.size.width,
                    menu_base_y,
                    menu_item_h,
                    self.state.prev_menu_selected,
                    menu_items[self.state.prev_menu_selected].label,
                    false,
                    menu_bg,
                    &self.palette,
                );
                draw_menu_item(
                    disp,
                    layout.menu_rect.top_left.x,
                    layout.menu_rect.size.width,
                    menu_base_y,
                    menu_item_h,
                    ui.menu_selected,
                    menu_items[ui.menu_selected].label,
                    true,
                    menu_bg,
                    &self.palette,
                );
            }
        }

        if plan.dirty.content {
            let from_or_to_logs =
                matches!(page, PageId::Logs) || matches!(self.state.prev_page, PageId::Logs);
            let mut clear_rect = Rectangle::new(
                Point::new(layout.content_rect.top_left.x, layout.content_rect.top_left.y),
                Size::new(
                    disp.bounding_box()
                        .size
                        .width
                        .saturating_sub(layout.content_rect.top_left.x as u32),
                    layout.content_rect.size.height,
                ),
            );
            if from_or_to_logs || !self.state.bg_drawn {
                clear_rect.size.height = screen_h;
            } else {
                clear_rect.size.height = screen_h.min(140);
            }
            let _ = clear_rect
                .into_styled(PrimitiveStyle::with_fill(bg_color))
                .draw(disp);

            registry.reset_all();
        }

        let menu_right = layout.menu_clear_rect.size.width as i32;
        let content_left = layout.content_rect.top_left.x;
        if content_left > menu_right {
            let gap_width = (content_left - menu_right) as u32;
            let _ = Rectangle::new(Point::new(menu_right, 0), Size::new(gap_width, screen_h))
                .into_styled(PrimitiveStyle::with_fill(bg_color))
                .draw(disp);
        }

        let y_pos = layout.content_rect.top_left.y + 20;
        let line_x = layout.content_rect.top_left.x;
        let clear_w = disp
            .bounding_box()
            .size
            .width
            .saturating_sub(line_x as u32);

        if plan.dirty.page || plan.dirty.content {
            registry.render(
                page,
                PageRenderArgs {
                    disp,
                    line_x,
                    y_pos,
                    clear_w,
                    content_width: layout.content_width,
                    content_height: layout.content_rect.size.height,
                    bg_color,
                    config: &self.config,
                    title_style: &title_style,
                    body_style: &body_style,
                },
                page_data,
            );
        }

        self.state.prev_page = page;
        self.state.prev_menu_selected = ui.menu_selected;
        self.state.prev_menu_open = ui.menu_open;
        self.state.last_layout = Some(layout);
        self.state.bg_drawn = true;
    }
}

fn draw_menu_item(
    disp: &mut impl DrawTarget<Color = Rgb565>,
    menu_x: i32,
    menu_w: u32,
    base_y: i32,
    item_h: i32,
    idx: usize,
    label: &str,
    selected: bool,
    menu_bg: Rgb565,
    palette: &DisplayPalette,
) {
    let y = base_y + (idx as i32 * item_h);
    let sel_bg = if selected { palette.white } else { menu_bg };
    let sel_fg = if selected { palette.black } else { palette.white };
    let _ = Rectangle::new(
        Point::new(menu_x + 2, y - 9),
        Size::new(menu_w.saturating_sub(4), item_h as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(sel_bg))
    .draw(disp);
    let menu_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X9)
        .text_color(sel_fg)
        .background_color(sel_bg)
        .build();
    let _ = Text::new(label, Point::new(menu_x + 8, y), menu_style).draw(disp);
}

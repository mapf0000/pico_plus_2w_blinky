//! Incremental rendering. Cached pixels are independent of application models.

use embedded_graphics::{
    mono_font::MonoTextStyle,
    pixelcolor::{Rgb565, Rgb888},
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};
use heapless::Vec;

use super::{
    input::Navigation,
    model::{PageId, PageView},
    scene::{Font, MAX_ROWS, Row, Scene, TEXT_PAD, fit_text},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub white: Rgb565,
    pub yellow: Rgb565,
    pub teal: Rgb565,
    pub black: Rgb565,
    pub green: Rgb565,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            white: Rgb565::WHITE,
            yellow: Rgb888::new(0xF9, 0xDB, 0x6D).into(),
            teal: Rgb888::new(0x36, 0x82, 0x7F).into(),
            black: Rgb565::BLACK,
            green: Rgb888::new(0x00, 0x87, 0x61).into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayConfig {
    pub menu_width: u32,
    pub gutter: u32,
    pub divider_width: u32,
    pub content_padding: u32,
    pub palette: Palette,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            menu_width: 100,
            gutter: 4,
            divider_width: 2,
            content_padding: 10,
            palette: Palette::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    screen: Rectangle,
    menu: Rectangle,
    divider: Rectangle,
    content: Rectangle,
    viewport: Rectangle,
    menu_open: bool,
}

impl Layout {
    fn new(screen: Rectangle, config: DisplayConfig, menu_open: bool) -> Self {
        let width = screen.size.width;
        let menu_width = if menu_open {
            config.menu_width.min(width)
        } else {
            0
        };
        let divider_x = menu_width.saturating_add(config.gutter).min(width);
        let content_x = if menu_open {
            divider_x.saturating_add(config.divider_width).min(width)
        } else {
            0
        };
        let content_width = width - content_x;
        let padding = if menu_open {
            config.content_padding.min(content_width / 2)
        } else {
            0
        };
        let rect = |x, width| {
            Rectangle::new(
                screen.top_left + Point::new(x as i32, 0),
                Size::new(width, screen.size.height),
            )
        };
        Self {
            screen,
            menu: rect(0, menu_width),
            divider: rect(divider_x, content_x.saturating_sub(divider_x)),
            content: rect(content_x, content_width),
            viewport: rect(content_x + padding, content_width - padding * 2),
            menu_open,
        }
    }
}

pub struct Renderer {
    config: DisplayConfig,
    layout: Option<Layout>,
    page: Option<PageId>,
    menu_selected: usize,
    rows: Vec<Row, MAX_ROWS>,
}

impl Renderer {
    pub fn new(config: DisplayConfig) -> Self {
        Self {
            config,
            layout: None,
            page: None,
            menu_selected: 0,
            rows: Vec::new(),
        }
    }

    pub fn needs_layout(&self, screen: Rectangle, navigation: &Navigation, page: PageId) -> bool {
        self.layout != Some(Layout::new(screen, self.config, navigation.menu_open))
            || self.page != Some(page)
            || self.menu_selected != navigation.selected.0
    }

    pub fn invalidate(&mut self) {
        self.layout = None;
    }

    /// Prepare an owned frame so input/models can continue updating while hardware
    /// flushes it. Dropping a frame without committing leaves the cache invalid.
    pub fn prepare(
        &mut self,
        screen: Rectangle,
        navigation: &Navigation,
        view: PageView<'_>,
    ) -> Frame {
        let layout = Layout::new(screen, self.config, navigation.menu_open);
        let page = view.id();
        let geometry_changed = self.layout != Some(layout);
        let page_changed = self.page != Some(page);
        let selection_changed = self.menu_selected != navigation.selected.0;
        let mut frame = Frame {
            layout,
            page,
            menu_selected: navigation.selected.0,
            rows: Scene::build(view, layout.viewport, self.config.palette).rows,
            menu: Vec::new(),
            commands: Vec::new(),
        };
        self.layout = None;
        if navigation.menu_open {
            for (index, item) in PageId::ALL.iter().enumerate() {
                // Capacity equals the static page count.
                let _ = frame.menu.push(menu_row(
                    layout,
                    self.config.palette,
                    index,
                    item.menu_label(),
                    index == navigation.selected.0,
                ));
            }
        }
        if geometry_changed {
            if navigation.menu_open {
                frame.background(layout.menu, true, self.config.palette.black);
                let gap =
                    Rectangle::new(
                        layout.menu.top_left + Point::new(layout.menu.size.width as i32, 0),
                        Size::new(
                            layout.divider.top_left.x.saturating_sub(
                                layout.menu.top_left.x + layout.menu.size.width as i32,
                            ) as u32,
                            screen.size.height,
                        ),
                    );
                frame.fill(gap, self.config.palette.black);
                frame.fill(layout.divider, self.config.palette.white);
                for index in 0..frame.menu.len() {
                    frame.command(Command::Row { index, menu: true });
                }
            }
        } else if selection_changed && navigation.menu_open {
            for index in [self.menu_selected, navigation.selected.0] {
                if index < frame.menu.len() {
                    frame.command(Command::Row { index, menu: true });
                }
            }
        }
        if geometry_changed || page_changed {
            // Paint each pixel once on a full redraw: backgrounds exclude rows.
            frame.background(layout.content, false, self.config.palette.black);
        } else {
            for (index, old) in self.rows.iter().enumerate() {
                if frame.rows.get(index).is_none_or(|row| row.rect != old.rect) {
                    frame.fill(old.rect, self.config.palette.black);
                }
            }
        }
        for index in 0..frame.rows.len() {
            let row = &frame.rows[index];
            let erased = frame.commands.iter().any(|command| match command {
                Command::Fill(rect, _) => rect.intersection(&row.rect).size != Size::zero(),
                Command::Row { .. } => false,
            });
            if geometry_changed || page_changed || erased || self.rows.get(index) != Some(row) {
                frame.command(Command::Row { index, menu: false });
            }
        }
        frame
    }

    /// Commit only after every hardware write succeeds.
    pub fn commit(&mut self, frame: Frame) {
        self.layout = Some(frame.layout);
        self.page = Some(frame.page);
        self.menu_selected = frame.menu_selected;
        self.rows = frame.rows;
    }

    #[cfg(not(target_os = "none"))]
    pub fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        navigation: &Navigation,
        view: PageView<'_>,
    ) -> Result<bool, D::Error> {
        let frame = self.prepare(target.bounding_box(), navigation, view);
        let drew = !frame.commands.is_empty();
        for paint in frame.paints() {
            paint.draw(target)?;
        }
        self.commit(frame);
        Ok(drew)
    }
}

// Worst case: each row needs a background above it and two side strips, plus
// its paint; menu has the same bound, with room for old-row erasure and shell.
const MAX_COMMANDS: usize = MAX_ROWS * 5 + PageId::ALL.len() * 4 + 8;

#[derive(Clone, Copy)]
enum Command {
    Fill(Rectangle, Rgb565),
    Row { index: usize, menu: bool },
}

/// Bounded redraw work. It owns its text and never borrows application models.
pub struct Frame {
    layout: Layout,
    page: PageId,
    menu_selected: usize,
    rows: Vec<Row, MAX_ROWS>,
    menu: Vec<Row, { PageId::ALL.len() }>,
    commands: Vec<Command, MAX_COMMANDS>,
}

impl Frame {
    fn command(&mut self, command: Command) {
        // MAX_COMMANDS bounds all rows, erased rows, margins, and shell above.
        assert!(self.commands.push(command).is_ok());
    }

    fn fill(&mut self, rect: Rectangle, color: Rgb565) {
        let rect = rect.intersection(&self.layout.screen);
        if rect.size.width != 0 && rect.size.height != 0 {
            self.command(Command::Fill(rect, color));
        }
    }

    fn background(&mut self, area: Rectangle, menu: bool, color: Rgb565) {
        let mut y = area.top_left.y;
        let end = y + area.size.height as i32;
        let count = if menu {
            self.menu.len()
        } else {
            self.rows.len()
        };
        for index in 0..count {
            let rect = if menu {
                self.menu[index].rect
            } else {
                self.rows[index].rect
            };
            let row = rect.intersection(&area);
            if row.size.width == 0 || row.size.height == 0 {
                continue;
            }
            self.fill(
                Rectangle::new(
                    Point::new(area.top_left.x, y),
                    Size::new(area.size.width, row.top_left.y.saturating_sub(y) as u32),
                ),
                color,
            );
            let right = row.top_left.x + row.size.width as i32;
            self.fill(
                Rectangle::new(
                    Point::new(area.top_left.x, row.top_left.y),
                    Size::new((row.top_left.x - area.top_left.x) as u32, row.size.height),
                ),
                color,
            );
            self.fill(
                Rectangle::new(
                    Point::new(right, row.top_left.y),
                    Size::new(
                        (area.top_left.x + area.size.width as i32 - right) as u32,
                        row.size.height,
                    ),
                ),
                color,
            );
            y = y.max(row.top_left.y + row.size.height as i32);
        }
        self.fill(
            Rectangle::new(
                Point::new(area.top_left.x, y),
                Size::new(area.size.width, end.saturating_sub(y) as u32),
            ),
            color,
        );
    }

    pub fn paints(&self) -> impl Iterator<Item = Paint<'_>> {
        self.commands
            .iter()
            .map(|command| match *command {
                Command::Fill(rect, color) => Paint::Solid(rect, color),
                Command::Row { index, menu } => {
                    let row = if menu {
                        &self.menu[index]
                    } else {
                        &self.rows[index]
                    };
                    let clip = if menu {
                        self.layout.menu
                    } else {
                        self.layout.viewport
                    };
                    Paint::Row(
                        row,
                        row.rect
                            .intersection(&clip)
                            .intersection(&self.layout.screen),
                    )
                }
            })
            .filter(|paint| paint.bounds().size.width != 0 && paint.bounds().size.height != 0)
    }
}

pub enum Paint<'a> {
    Solid(Rectangle, Rgb565),
    Row(&'a Row, Rectangle),
}

impl Paint<'_> {
    pub fn bounds(&self) -> Rectangle {
        match self {
            Self::Solid(rect, _) | Self::Row(_, rect) => *rect,
        }
    }

    pub fn draw<D: DrawTarget<Color = Rgb565>>(&self, target: &mut D) -> Result<(), D::Error> {
        let mut target = target.clipped(&self.bounds());
        match self {
            Self::Solid(rect, color) => target.fill_solid(rect, *color),
            Self::Row(row, _) => draw_row(&mut target, row),
        }
    }
}

fn menu_row(layout: Layout, palette: Palette, index: usize, label: &str, selected: bool) -> Row {
    let rect = Rectangle::new(
        layout.menu.top_left + Point::new(2, 11 + index as i32 * 18),
        Size::new(layout.menu.size.width.saturating_sub(4), 18),
    );
    let columns =
        rect.size.width.saturating_sub(TEXT_PAD * 2) / Font::Body.metrics().character_size.width;
    Row {
        rect,
        text: fit_text(label, columns as usize),
        font: Font::Body,
        foreground: if selected {
            palette.black
        } else {
            palette.white
        },
        background: if selected {
            palette.white
        } else {
            palette.black
        },
    }
}

fn draw_row<D: DrawTarget<Color = Rgb565>>(target: &mut D, row: &Row) -> Result<(), D::Error> {
    let mut target = target.clipped(&row.rect);
    target.fill_solid(&row.rect, row.background)?;
    let font = row.font.metrics();
    let mut style = MonoTextStyle::new(font, row.foreground);
    // Opaque glyphs use bulk fill_contiguous, and selected text inherits the row
    // background instead of silently keeping the normal page background.
    style.background_color = Some(row.background);
    let y = (row
        .rect
        .size
        .height
        .saturating_sub(font.character_size.height)
        / 2) as i32;
    Text::with_baseline(
        &row.text,
        row.rect.top_left + Point::new(TEXT_PAD as i32, y),
        style,
        Baseline::Top,
    )
    .draw(&mut target)?;
    Ok(())
}

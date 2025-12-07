use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use super::page_common::draw_placeholder_page;

pub fn render(
    disp: &mut impl DrawTarget<Color = Rgb565>,
    line_x: i32,
    y_pos: i32,
    title_style: &MonoTextStyle<Rgb565>,
    body_style: &MonoTextStyle<Rgb565>,
) {
    draw_placeholder_page(
        disp,
        line_x,
        y_pos,
        "Item 1",
        "Placeholder content for Item 1.",
        title_style,
        body_style,
    );
}

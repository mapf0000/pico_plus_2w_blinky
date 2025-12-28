use core::fmt::Write as _;

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use embassy_time::Duration;
use heapless::String;

pub const TEXT_PAD: i32 = 6;

pub fn build_text_line<const N: usize>(prefix: &str, value: &str) -> String<N> {
    let mut line: String<N> = String::new();
    let _ = write!(line, "{}{}", prefix, value);
    line
}

pub fn update_line<const N: usize>(
    disp: &mut impl DrawTarget<Color = Rgb565>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    bg: Rgb565,
    text: &str,
    prev: &mut String<N>,
    style: &MonoTextStyle<Rgb565>,
) {
    if prev.as_str() != text {
        let _ = Rectangle::new(Point::new(x, y - 11), Size::new(w, h))
            .into_styled(PrimitiveStyle::with_fill(bg))
            .draw(disp);
        let _ = Text::new(text, Point::new(x + TEXT_PAD, y), *style).draw(disp);
        prev.clear();
        let _ = prev.push_str(text);
    }
}

pub fn format_hms(duration: Duration) -> String<20> {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;

    let mut out: String<20> = String::new();
    if hours >= 24 {
        let days = hours / 24;
        let _ = write!(out, "{}d {:02}:{:02}:{:02}", days, hours % 24, minutes, seconds);
    } else {
        let _ = write!(out, "{:02}:{:02}:{:02}", hours, minutes, seconds);
    }
    out
}

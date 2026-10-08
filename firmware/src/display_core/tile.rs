//! A borrowed RGB565 tile; its bytes are already in the panel's wire order.

use core::convert::Infallible;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

pub const TILE_WIDTH: u32 = 320;
pub const TILE_HEIGHT: u32 = 8;
pub const TILE_BYTES: usize = (TILE_WIDTH * TILE_HEIGHT * 2) as usize;

pub struct Tile<'a> {
    bounds: Rectangle,
    bytes: &'a mut [u8],
}

impl<'a> Tile<'a> {
    pub fn new(bounds: Rectangle, buffer: &'a mut [u8]) -> Option<Self> {
        let length = (bounds.size.width as usize)
            .checked_mul(bounds.size.height as usize)?
            .checked_mul(2)?;
        let bytes = buffer.get_mut(..length)?;
        Some(Self { bounds, bytes })
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }

    fn offset(&self, point: Point) -> Option<usize> {
        if !self.bounds.contains(point) {
            return None;
        }
        let point = point - self.bounds.top_left;
        Some((point.y as usize * self.bounds.size.width as usize + point.x as usize) * 2)
    }
}

impl Dimensions for Tile<'_> {
    fn bounding_box(&self) -> Rectangle {
        self.bounds
    }
}

impl DrawTarget for Tile<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if let Some(offset) = self.offset(point) {
                self.bytes[offset..offset + 2].copy_from_slice(&color.into_storage().to_be_bytes());
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounds);
        let color = color.into_storage().to_be_bytes();
        for y in 0..area.size.height {
            let point = area.top_left + Point::new(0, y as i32);
            if let Some(offset) = self.offset(point) {
                for pixel in self.bytes[offset..offset + area.size.width as usize * 2]
                    .as_chunks_mut::<2>()
                    .0
                {
                    pixel.copy_from_slice(&color);
                }
            }
        }
        Ok(())
    }
}

/// Split a damaged rectangle into bounded full-width stripes, preserving origin.
pub fn tiles(area: Rectangle) -> impl Iterator<Item = Rectangle> {
    (0..area.size.height)
        .step_by(TILE_HEIGHT as usize)
        .map(move |y| {
            Rectangle::new(
                area.top_left + Point::new(0, y as i32),
                Size::new(area.size.width, TILE_HEIGHT.min(area.size.height - y)),
            )
        })
}

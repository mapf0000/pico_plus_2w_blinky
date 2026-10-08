//! Board-specific ST7789 commands and asynchronous, single-buffer DMA transport.

use core::sync::atomic::{AtomicU32, Ordering};
use embassy_rp::{
    Peri,
    gpio::{Level, Output},
    peripherals::{DMA_CH1, PIN_16, PIN_17, PIN_18, PIN_19, SPI0},
    spi::{Async, Phase, Polarity, Spi},
};
use embassy_time::{Instant, Timer};
use embedded_graphics::{prelude::*, primitives::Rectangle};
use static_cell::StaticCell;

use crate::display_core::{
    renderer::Frame,
    tile::{TILE_BYTES, TILE_WIDTH, Tile, tiles},
};

pub(super) struct DisplayBackendPins<'d> {
    pub spi: Peri<'d, SPI0>,
    pub dma: Peri<'d, DMA_CH1>,
    pub sck: Peri<'d, PIN_18>,
    pub mosi: Peri<'d, PIN_19>,
    pub cs: Peri<'d, PIN_17>,
    pub dc: Peri<'d, PIN_16>,
}

pub(super) struct Display {
    spi: Spi<'static, SPI0, Async>,
    cs: Output<'static>,
    dc: Output<'static>,
    buffer: &'static mut [u8; TILE_BYTES],
}

impl Dimensions for Display {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(Point::zero(), Size::new(TILE_WIDTH, 240))
    }
}

/// Restore CS even if an in-flight command is dropped or fails. The sole owner
/// retains the DMA buffer until its write completes; no shared bus or unsafe access.
struct Selected<'a>(&'a mut Output<'static>);
impl Drop for Selected<'_> {
    fn drop(&mut self) {
        self.0.set_high();
    }
}

static SPI_BYTES: AtomicU32 = AtomicU32::new(0);
static SPI_WRITES: AtomicU32 = AtomicU32::new(0);
static MAX_RASTER_US: AtomicU32 = AtomicU32::new(0);

pub(super) fn max_raster_us() -> u32 {
    MAX_RASTER_US.load(Ordering::Relaxed)
}
pub(super) fn spi_counters() -> (u32, u32) {
    (
        SPI_BYTES.load(Ordering::Relaxed),
        SPI_WRITES.load(Ordering::Relaxed),
    )
}

async fn write(
    spi: &mut Spi<'static, SPI0, Async>,
    bytes: &[u8],
) -> Result<(), embassy_rp::spi::Error> {
    SPI_BYTES.fetch_add(bytes.len() as u32, Ordering::Relaxed);
    SPI_WRITES.fetch_add(1, Ordering::Relaxed);
    spi.write(bytes).await
}

async fn command(
    spi: &mut Spi<'static, SPI0, Async>,
    cs: &mut Output<'static>,
    dc: &mut Output<'static>,
    command: u8,
    data: &[u8],
) -> Result<(), embassy_rp::spi::Error> {
    cs.set_low();
    let _selected = Selected(cs);
    dc.set_low();
    write(spi, &[command]).await?;
    dc.set_high();
    if !data.is_empty() {
        write(spi, data).await?;
    }
    Ok(())
}

impl Display {
    async fn command(&mut self, code: u8, data: &[u8]) -> Result<(), embassy_rp::spi::Error> {
        command(&mut self.spi, &mut self.cs, &mut self.dc, code, data).await
    }

    /// Preserve Pimoroni's mode-0, RGB565, landscape (MADCTL 0x70) setup.
    async fn initialize(&mut self) -> Result<(), embassy_rp::spi::Error> {
        self.command(0x01, &[]).await?; // SWRESET
        Timer::after_millis(150).await;
        for (code, params) in [
            (0x35, &[0x00][..]),                         // TEON; output pin is unused
            (0x3A, &[0x55][..]),                         // COLMOD
            (0xB2, &[0x0c, 0x0c, 0x00, 0x33, 0x33][..]), // PORCTRL
            (0xC0, &[0x2c][..]),                         // LCMCTRL
            (0xC2, &[0x01][..]),                         // VDVVRHEN
            (0xC3, &[0x12][..]),                         // VRHS
            (0xC4, &[0x20][..]),                         // VDVS
            (0xD0, &[0xA4, 0xA1][..]),                   // PWCTRL1
            (0xC6, &[0x0f][..]),                         // FRCTRL2
            (0xB0, &[0x00, 0xC0][..]),                   // RAMCTRL (banding fix)
            (0xB7, &[0x35][..]),                         // GCTRL
            (0xBB, &[0x1f][..]),                         // VCOMS
            (
                0xE0,
                &[
                    0xD0, 0x08, 0x11, 0x08, 0x0C, 0x15, 0x39, 0x33, 0x50, 0x36, 0x13, 0x14, 0x29,
                    0x2D,
                ][..],
            ),
            (
                0xE1,
                &[
                    0xD0, 0x08, 0x10, 0x08, 0x06, 0x06, 0x39, 0x44, 0x51, 0x0B, 0x16, 0x14, 0x2F,
                    0x31,
                ][..],
            ),
            (0x36, &[0x70][..]), // MADCTL: XY swap, column order, bottom-to-top
            (0x3A, &[0x55][..]), // 16-bit color
            (0x21, &[][..]),     // INVON
            (0x11, &[][..]),     // SLPOUT
        ] {
            self.command(code, params).await?;
        }
        Timer::after_millis(120).await;
        self.command(0x13, &[]).await?; // NORON
        Timer::after_millis(10).await;
        self.command(0x29, &[]).await?; // DISPON
        Timer::after_millis(120).await;
        Ok(())
    }

    pub(super) async fn flush(&mut self, frame: &Frame) -> Result<bool, embassy_rp::spi::Error> {
        let mut drew = false;
        for paint in frame.paints() {
            for area in tiles(paint.bounds()) {
                // Frame bounds are clipped to the 320x240 panel; stripes are <=8 rows.
                let raster_started = Instant::now();
                let mut tile = Tile::new(area, self.buffer).expect("panel tile fits static buffer");
                match paint.draw(&mut tile) {
                    Ok(()) => {}
                    Err(never) => match never {},
                }
                let length = tile.bytes().len();
                MAX_RASTER_US.fetch_max(
                    Instant::now()
                        .duration_since(raster_started)
                        .as_micros()
                        .min(u32::MAX as u64) as u32,
                    Ordering::Relaxed,
                );
                let x = area.top_left.x as u16;
                let y = area.top_left.y as u16;
                let end_x = x + area.size.width as u16 - 1;
                let end_y = y + area.size.height as u16 - 1;
                self.command(
                    0x2A,
                    &[(x >> 8) as u8, x as u8, (end_x >> 8) as u8, end_x as u8],
                )
                .await?;
                self.command(
                    0x2B,
                    &[(y >> 8) as u8, y as u8, (end_y >> 8) as u8, end_y as u8],
                )
                .await?;
                // Borrowing distinct fields prevents buffer mutation during DMA.
                command(
                    &mut self.spi,
                    &mut self.cs,
                    &mut self.dc,
                    0x2C,
                    &self.buffer[..length],
                )
                .await?;
                drew = true;
            }
        }
        Ok(drew)
    }
}

pub(super) async fn init(
    pins: DisplayBackendPins<'static>,
    backlight: &mut Output<'static>,
) -> Option<Display> {
    let mut config = embassy_rp::spi::Config::default();
    config.frequency = 62_500_000;
    config.phase = Phase::CaptureOnFirstTransition;
    config.polarity = Polarity::IdleLow;
    static BUFFER: StaticCell<[u8; TILE_BYTES]> = StaticCell::new();
    let mut display = Display {
        spi: Spi::new_txonly(pins.spi, pins.sck, pins.mosi, pins.dma, crate::Irqs, config),
        cs: Output::new(pins.cs, Level::High),
        dc: Output::new(pins.dc, Level::High),
        buffer: BUFFER.init([0; TILE_BYTES]),
    };
    let initialized = display.initialize().await;
    backlight.set_high();
    match initialized {
        Ok(()) => {
            log::info!("display: ST7789 init ok (mode0, 62.5MHz, 320x240, DMA_CH1, 8-row tile)");
            Some(display)
        }
        Err(error) => {
            log::warn!(
                "display: ST7789 init failed: {:?}; continuing with buttons only",
                error
            );
            None
        }
    }
}

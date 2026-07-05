use embassy_rp::Peri;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{PIN_16, PIN_17, PIN_18, PIN_19, SPI0};
use embassy_rp::spi::{Blocking, Phase, Polarity, Spi};
use embassy_time::Delay;
use embedded_graphics::geometry::Dimensions;
use embedded_hal::delay::DelayNs;
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::Builder;
use mipidsi::dcs::{
    BitsPerPixel, EnterNormalMode, ExitSleepMode, PixelFormat, SetAddressMode, SetDisplayOn,
    SetPixelFormat,
};
use mipidsi::dcs::{DcsCommand, InterfaceExt};
use mipidsi::interface::SpiInterface;
use mipidsi::models::{Model, ModelInitError};
use mipidsi::options::{
    ColorOrder, HorizontalRefreshOrder, ModelOptions, Orientation, RefreshOrder, Rotation,
    VerticalRefreshOrder,
};
use static_cell::StaticCell;

use embedded_graphics::pixelcolor::Rgb565;

pub(super) struct DisplayBackendPins<'d> {
    pub spi: Peri<'d, SPI0>,
    pub sck: Peri<'d, PIN_18>,
    pub mosi: Peri<'d, PIN_19>,
    pub cs: Peri<'d, PIN_17>,
    pub dc: Peri<'d, PIN_16>,
}

type SpiBus = Spi<'static, SPI0, Blocking>;
type SpiDevice = ExclusiveDevice<SpiBus, Output<'static>, Delay>;
type DisplayInterface = SpiInterface<'static, SpiDevice, Output<'static>>;

pub(super) type Display = mipidsi::Display<DisplayInterface, St7789Pico28, mipidsi::NoResetPin>;

/// ST7789 init sequence tuned for Pimoroni Pico Display 2.8 (320x240).
pub(super) struct St7789Pico28;

impl Model for St7789Pico28 {
    type ColorFormat = Rgb565;
    const FRAMEBUFFER_SIZE: (u16, u16) = (240, 320);

    fn init<DELAY, DI>(
        &mut self,
        di: &mut DI,
        delay: &mut DELAY,
        options: &ModelOptions,
    ) -> Result<SetAddressMode, ModelInitError<DI::Error>>
    where
        DELAY: DelayNs,
        DI: mipidsi::interface::Interface,
    {
        // Power-on/reset sequence mirrors Pimoroni's C driver.
        di.send_command(0x01, &[])?; // SWRESET
        delay.delay_us(150_000);

        di.send_command(0x35, &[0x00])?; // TEON (tearing-effect output disabled)
        di.send_command(0x3A, &[0x55])?; // COLMOD: 16-bit color
        di.send_command(0xB2, &[0x0c, 0x0c, 0x00, 0x33, 0x33])?; // PORCTRL
        di.send_command(0xC0, &[0x2c])?; // LCMCTRL
        di.send_command(0xC2, &[0x01])?; // VDVVRHEN
        di.send_command(0xC3, &[0x12])?; // VRHS
        di.send_command(0xC4, &[0x20])?; // VDVS
        di.send_command(0xD0, &[0xA4, 0xA1])?; // PWCTRL1
        di.send_command(0xC6, &[0x0f])?; // FRCTRL2
        di.send_command(0xB0, &[0x00, 0xC0])?; // RAMCTRL (banding fix)
        di.send_command(0xB7, &[0x35])?; // GCTRL for 320x240
        di.send_command(0xBB, &[0x1f])?; // VCOMS for 320x240
        di.send_command(
            0xE0,
            &[
                0xD0, 0x08, 0x11, 0x08, 0x0C, 0x15, 0x39, 0x33, 0x50, 0x36, 0x13, 0x14, 0x29, 0x2D,
            ],
        )?; // GMCTRP1 (positive gamma)
        di.send_command(
            0xE1,
            &[
                0xD0, 0x08, 0x10, 0x08, 0x06, 0x06, 0x39, 0x44, 0x51, 0x0B, 0x16, 0x14, 0x2F, 0x31,
            ],
        )?; // GMCTRN1 (negative gamma)

        // Pimoroni config for 320x240 @ ROTATE_0 sets MADCTL = COL_ORDER | SWAP_XY | SCAN_ORDER (0x70).
        let madctl = SetAddressMode::from(options).with_refresh_order(RefreshOrder::new(
            VerticalRefreshOrder::BottomToTop,
            HorizontalRefreshOrder::LeftToRight,
        ));
        let mut madctl_bytes = [0u8; 1];
        madctl.fill_params_buf(&mut madctl_bytes);
        log::info!("display: MADCTL=0x{:02x}", madctl_bytes[0]);
        di.write_command(madctl)?; // orientation + color order + refresh direction

        let pf = PixelFormat::with_all(BitsPerPixel::from_rgb_color::<Self::ColorFormat>());
        di.write_command(SetPixelFormat::new(pf))?;

        di.send_command(0x21, &[])?; // INVON (matches Pimoroni defaults)
        di.write_command(ExitSleepMode)?;
        delay.delay_us(120_000);
        di.write_command(EnterNormalMode)?;
        delay.delay_us(10_000);
        di.write_command(SetDisplayOn)?;
        delay.delay_us(120_000);

        Ok(madctl)
    }
}

pub(super) fn init(
    pins: DisplayBackendPins<'static>,
    backlight: &mut Output<'static>,
) -> Option<Display> {
    let mut spi_cfg = embassy_rp::spi::Config::default();
    spi_cfg.frequency = 62_500_000; // matches Pimoroni driver for RP2040/2350
    // Pimoroni ships this panel in SPI mode 0 (CPOL=0, CPHA=0).
    spi_cfg.phase = Phase::CaptureOnFirstTransition;
    spi_cfg.polarity = Polarity::IdleLow;

    let bus = Spi::new_blocking_txonly(pins.spi, pins.sck, pins.mosi, spi_cfg);
    let cs = Output::new(pins.cs, Level::High);
    let dc = Output::new(pins.dc, Level::High);

    static SPI_BUFFER: StaticCell<[u8; 512]> = StaticCell::new();
    let buf = SPI_BUFFER.init([0u8; 512]);

    match ExclusiveDevice::new(bus, cs, Delay) {
        Ok(device) => {
            let interface = SpiInterface::new(device, dc, buf);

            // ST7789 framebuffer is 240x320; rotate to get a 320x240 layout.
            // Match Pimoroni rotation (MADCTL 0x70 => swap XY + column order + bottom-to-top).
            let orientation = Orientation::new().rotate(Rotation::Deg90);
            let mut delay = Delay;
            match Builder::new(St7789Pico28, interface)
                .display_size(240, 320)
                .orientation(orientation)
                .color_order(ColorOrder::Rgb)
                .init(&mut delay)
            {
                Ok(disp) => {
                    backlight.set_high();
                    let bbox = disp.bounding_box();
                    log::info!(
                        "display: ST7789 init ok (mode0, 62.5MHz, bbox {}x{} @ rot0)",
                        bbox.size.width,
                        bbox.size.height
                    );
                    Some(disp)
                }
                Err(e) => {
                    log::warn!(
                        "display: ST7789 init failed: {:?}; continuing with buttons only",
                        e
                    );
                    backlight.set_high(); // at least turn on backlight
                    None
                }
            }
        }
        Err(_) => {
            log::warn!("display: SPI device setup failed; continuing with buttons only");
            backlight.set_high();
            None
        }
    }
}

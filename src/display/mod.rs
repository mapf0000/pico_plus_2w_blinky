use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_rp::adc::{Adc, Channel, Config as AdcConfig};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{
    ADC, ADC_TEMP_SENSOR, PIN_12, PIN_13, PIN_14, PIN_15, PIN_16, PIN_17, PIN_18, PIN_19, PIN_20,
    PIN_26, PIN_27, PIN_28, SPI0,
};
use embassy_rp::Peri;
use embassy_rp::spi::{Phase, Polarity, Spi};
use embassy_time::{Delay, Duration, Instant, Ticker};
use embedded_graphics::mono_font::ascii::{FONT_6X9, FONT_8X13};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::{Rgb565, Rgb888};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use embedded_hal::delay::DelayNs;
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::String;
use mipidsi::dcs::{
    BitsPerPixel, EnterNormalMode, ExitSleepMode, PixelFormat, SetAddressMode, SetDisplayOn,
    SetPixelFormat,
};
use mipidsi::dcs::{DcsCommand, InterfaceExt};
use mipidsi::interface::SpiInterface;
use mipidsi::models::Model;
use mipidsi::options::{
    ColorOrder, HorizontalRefreshOrder, ModelOptions, Orientation, RefreshOrder, Rotation,
    VerticalRefreshOrder,
};
use mipidsi::Builder;
use static_cell::StaticCell;

use crate::log_buffer;

mod page_common;
mod page_daemon;
mod page_payloads;
mod page_logs;
mod page_system;

use page_common::{build_text_line, format_hms};
use page_daemon::DaemonPageState;
use page_payloads::PayloadsPageState;
use page_logs::LogsPageState;
use page_system::SystemPageState;

bind_interrupts!(struct AdcIrqs {
    ADC_IRQ_FIFO => embassy_rp::adc::InterruptHandler;
});

// Pico Display 2.8 uses active-low RGB LED channels.
pub struct DisplayPins<'d> {
    pub adc: Peri<'d, ADC>,
    pub temp_sensor: Peri<'d, ADC_TEMP_SENSOR>,
    pub spi: Peri<'d, SPI0>,
    pub sck: Peri<'d, PIN_18>,
    pub mosi: Peri<'d, PIN_19>,
    pub cs: Peri<'d, PIN_17>,
    pub dc: Peri<'d, PIN_16>,
    pub backlight: Peri<'d, PIN_20>,
    pub btn_a: Peri<'d, PIN_12>,
    pub btn_b: Peri<'d, PIN_13>,
    pub btn_x: Peri<'d, PIN_14>,
    pub btn_y: Peri<'d, PIN_15>,
    pub led_r: Peri<'d, PIN_26>,
    pub led_g: Peri<'d, PIN_27>,
    pub led_b: Peri<'d, PIN_28>,
    pub ap_ssid: &'static str,
}

#[derive(Clone, Copy)]
pub struct DisplayPalette {
    pub white: Rgb565,
    pub yellow: Rgb565,
    pub teal: Rgb565,
    pub blue: Rgb565,
    pub black: Rgb565,
    pub green: Rgb565,
}

impl DisplayPalette {
    pub fn new() -> Self {
        Self {
            white: Rgb565::from(Rgb888::new(0xFF, 0xFF, 0xFF)),
            yellow: Rgb565::from(Rgb888::new(0xF9, 0xDB, 0x6D)),
            teal: Rgb565::from(Rgb888::new(0x36, 0x82, 0x7F)),
            blue: Rgb565::from(Rgb888::new(0x46, 0x4D, 0x77)),
            black: Rgb565::from(Rgb888::new(0x00, 0x00, 0x00)),
            green: Rgb565::from(Rgb888::new(0x00, 0x87, 0x61)),
        }
    }
}

impl Default for DisplayPalette {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Payloads,
    Daemon,
    System,
    Logs,
}

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

#[derive(Clone, Copy)]
pub struct DisplayConfig {
    pub menu_width: u32,
    pub gutter: u32,
    pub divider_width: u32,
    pub content_padding: i32,
    pub header_bg: Rgb565,
    pub header_text: Rgb565,
    pub palette: DisplayPalette,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        let palette = DisplayPalette::default();
        Self {
            menu_width: 100,
            gutter: 4,
            divider_width: 2,
            content_padding: 10,
            header_bg: palette.teal,
            header_text: palette.white,
            palette,
        }
    }
}

pub struct LedColor {
    pub name: &'static str,
    pub r: bool,
    pub g: bool,
    pub b: bool,
}

pub const LED_COLORS: &[LedColor] = &[
    LedColor {
        name: "off",
        r: false,
        g: false,
        b: false,
    },
    LedColor {
        name: "red",
        r: true,
        g: false,
        b: false,
    },
    LedColor {
        name: "green",
        r: false,
        g: true,
        b: false,
    },
    LedColor {
        name: "blue",
        r: false,
        g: false,
        b: true,
    },
    LedColor {
        name: "white",
        r: true,
        g: true,
        b: true,
    },
];

fn adc_temp_to_celsius(raw: u16) -> f32 {
    // Datasheet formula: 27 - (v_sense - 0.706) / 0.001721; v_sense derived from 12-bit ADC.
    const VREF: f32 = 3.3;
    let voltage = (raw as f32) * VREF / 4096.0;
    27.0 - (voltage - 0.706) / 0.001721
}

fn psram_status_line() -> String<32> {
    #[cfg(feature = "psram")]
    {
        let status = if crate::psram_pool::http_buffers().is_some() {
            "ok"
        } else {
            "not detected"
        };
        return build_text_line("PSRAM: ", status);
    }

    #[cfg(not(feature = "psram"))]
    {
        build_text_line("PSRAM: ", "disabled")
    }
}

fn firmware_size_bytes() -> usize {
    unsafe extern "C" {
        static __start_block_addr: u8;
        static __end_block_addr: u8;
    }
    let start = core::ptr::addr_of!(__start_block_addr) as usize;
    let end = core::ptr::addr_of!(__end_block_addr) as usize;
    end.saturating_sub(start)
}

fn format_bytes_mb_one_decimal(bytes: usize) -> String<16> {
    const MIB: usize = 1024 * 1024;
    let whole = bytes / MIB;
    let tenths = ((bytes % MIB) * 10) / MIB;
    let mut s: String<16> = String::new();
    let _ = write!(s, "{}.{}MB", whole, tenths);
    s
}

fn flash_status_line(total: usize, free: usize) -> String<48> {
    let used = total.saturating_sub(free);
    let pct_free = if total > 0 {
        (free * 100) / total
    } else {
        0
    };
    let total_s = format_bytes_mb_one_decimal(total);
    let used_s = format_bytes_mb_one_decimal(used);
    let free_s = format_bytes_mb_one_decimal(free);
    let mut line: String<48> = String::new();
    let _ = write!(
        line,
        "Flash: {} used / {} free ({}%)",
        used_s.as_str(),
        free_s.as_str(),
        pct_free
    );
    let _ = write!(line, " tot {}", total_s.as_str());
    line
}

const MENU_ITEMS: [(Page, &str); 4] = [
    (Page::Payloads, "Payloads"),
    (Page::Daemon, "Host Agent"),
    (Page::System, "System"),
    (Page::Logs, "Logs"),
];

pub fn spawn(spawner: &Spawner, pins: DisplayPins<'static>) -> bool {
    crate::log_spawn(spawner, "display_task", display_task(pins))
}

/// ST7789 init sequence tuned for Pimoroni Pico Display 2.8 (320x240).
struct St7789Pico28;

impl Model for St7789Pico28 {
    type ColorFormat = Rgb565;
    const FRAMEBUFFER_SIZE: (u16, u16) = (240, 320);

    fn init<DELAY, DI>(
        &mut self,
        di: &mut DI,
        delay: &mut DELAY,
        options: &ModelOptions,
    ) -> Result<SetAddressMode, DI::Error>
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

fn apply_led(
    color: &LedColor,
    r: &mut Output<'static>,
    g: &mut Output<'static>,
    b: &mut Output<'static>,
) {
    // Active-low LED: low = on, high = off.
    let _ = if color.r { r.set_low() } else { r.set_high() };
    let _ = if color.g { g.set_low() } else { g.set_high() };
    let _ = if color.b { b.set_low() } else { b.set_high() };
}

#[inline]
fn debounce_button(raw: bool, state: &mut bool, counter: &mut u8) -> bool {
    if raw == *state {
        *counter = 0;
    } else {
        *counter = counter.saturating_add(1);
        if *counter >= 2 {
            *state = raw;
            *counter = 0;
        }
    }
    *state
}

fn menu_page(idx: usize) -> Page {
    MENU_ITEMS
        .get(idx)
        .map(|(page, _)| *page)
        .unwrap_or(Page::System)
}

fn default_menu_index() -> usize {
    MENU_ITEMS
        .iter()
        .position(|(page, _)| matches!(page, Page::Payloads))
        .unwrap_or(0)
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

struct PageState {
    payloads: PayloadsPageState,
    daemon: DaemonPageState,
    system: SystemPageState,
    logs: LogsPageState,
}

impl PageState {
    fn new() -> Self {
        Self {
            payloads: PayloadsPageState::new(),
            daemon: DaemonPageState::new(),
            system: SystemPageState::new(),
            logs: LogsPageState::new(),
        }
    }

    fn reset(&mut self) {
        self.payloads.reset();
        self.daemon.reset();
        self.system.reset();
        self.logs.reset();
    }
}

#[embassy_executor::task]
async fn display_task(pins: DisplayPins<'static>) -> ! {
    log::info!("display task starting (buttons + basic screen)");

    // LED + buttons are always available even if display init fails.
    let mut led_r = Output::new(pins.led_r, Level::High);
    let mut led_g = Output::new(pins.led_g, Level::High);
    let mut led_b = Output::new(pins.led_b, Level::High);

    let btn_a = Input::new(pins.btn_a, Pull::Up);
    let btn_b = Input::new(pins.btn_b, Pull::Up);
    let btn_x = Input::new(pins.btn_x, Pull::Up);
    let btn_y = Input::new(pins.btn_y, Pull::Up);

    // Backlight must stay driven; keep the pin alive for the whole task.
    let mut backlight = Output::new(pins.backlight, Level::High);

    let mut adc = Adc::new(pins.adc, AdcIrqs, AdcConfig::default());
    let mut temp_channel = Channel::new_temp_sensor(pins.temp_sensor);

    // --- Display init (best-effort; keep running even if it fails) ---
    let mut display: Option<_> = {
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
                        let _ = backlight.set_high();
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
                        let _ = backlight.set_high(); // at least turn on backlight
                        None
                    }
                }
            }
            Err(_) => {
                log::warn!("display: SPI device setup failed; continuing with buttons only");
                let _ = backlight.set_high();
                None
            }
        }
    };

    let mut color_idx = 0usize; // start off
    apply_led(&LED_COLORS[color_idx], &mut led_r, &mut led_g, &mut led_b);

    let mut prev_a = false;
    let mut prev_b = false;
    let mut prev_x = false;
    let mut prev_y = false;
    let mut a_db = false;
    let mut b_db = false;
    let mut x_db = false;
    let mut y_db = false;
    let mut a_db_count: u8 = 0;
    let mut b_db_count: u8 = 0;
    let mut x_db_count: u8 = 0;
    let mut y_db_count: u8 = 0;
    let mut bg_drawn = false;
    let mut page_state = PageState::new();
    let mut menu_selected: usize = default_menu_index();
    let mut prev_menu_selected = usize::MAX;
    let mut menu_open = false;
    let mut prev_menu_open = false;
    let mut nav_block = false;
    let mut ticker = Ticker::every(Duration::from_millis(50));
    let mut display_state_logged = false;
    let mut display_fail_logged = false;
    let mut prev_page = menu_page(menu_selected);
    let mut last_layout: Option<Layout> = None;
    let config = DisplayConfig::default();
    let palette = config.palette;
    let start_instant = Instant::now();
    let mut last_uptime_secs: u64 = 0;
    let flash_total = crate::device_config::FLASH_CAPACITY;
    let firmware_bytes = firmware_size_bytes();
    let flash_free = flash_total.saturating_sub(firmware_bytes);
    let flash_line = flash_status_line(flash_total, flash_free);

    loop {
        ticker.next().await;

        let a_pressed = debounce_button(btn_a.is_low(), &mut a_db, &mut a_db_count);
        let b_pressed = debounce_button(btn_b.is_low(), &mut b_db, &mut b_db_count);
        let x_pressed = debounce_button(btn_x.is_low(), &mut x_db, &mut x_db_count);
        let y_pressed = debounce_button(btn_y.is_low(), &mut y_db, &mut y_db_count);

        let nav_block_cleared = nav_block && !a_pressed && !b_pressed;
        if nav_block_cleared {
            nav_block = false;
        }

        let mut menu_toggled = false;
        if a_pressed && x_pressed && !prev_x {
            menu_open = !menu_open;
            menu_toggled = true;
            nav_block = true;
        }

        // Navigate on button release to avoid repeat while holding.
        if menu_open && !menu_toggled && !nav_block && !nav_block_cleared {
            if !a_pressed && prev_a {
                if menu_selected == 0 {
                    menu_selected = MENU_ITEMS.len() - 1;
                } else {
                    menu_selected -= 1;
                }
            }
            if !b_pressed && prev_b {
                menu_selected = (menu_selected + 1) % MENU_ITEMS.len();
            }
        }

        let y_released = !y_pressed && prev_y;
        if y_released && !matches!(menu_page(menu_selected), Page::Payloads) {
            color_idx = (color_idx + 1) % LED_COLORS.len();
            log::info!("Y press -> LED color {}", LED_COLORS[color_idx].name);
        }

        // If held, force LED to show activity immediately.
        let active_color_idx = color_idx;
        apply_led(
            &LED_COLORS[active_color_idx],
            &mut led_r,
            &mut led_g,
            &mut led_b,
        );

        // Render basic info if display is available.
        if let Some(ref mut disp) = display {
            if !display_state_logged {
                log::info!("display: ST7789 ready; entering render loop");
                display_state_logged = true;
            }

            let menu_changed = menu_open && menu_selected != prev_menu_selected;
            let menu_state_changed = menu_open != prev_menu_open;
            let page = menu_page(menu_selected);
            let page_changed = page != prev_page;
            let log_gen = if matches!(page, Page::Logs) {
                log_buffer::generation()
            } else {
                page_state.logs.last_gen
            };
            let log_changed = matches!(page, Page::Logs) && log_gen != page_state.logs.last_gen;
            let a_released = !a_pressed && prev_a;
            let b_released = !b_pressed && prev_b;
            let x_released = !x_pressed && prev_x;
            let mut dirty = Dirty::default();
            if !bg_drawn {
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
            if matches!(page, Page::System) {
                let uptime_secs = Instant::now()
                    .checked_duration_since(start_instant)
                    .unwrap_or_default()
                    .as_secs();
                if uptime_secs != last_uptime_secs {
                    dirty.page = true;
                    last_uptime_secs = uptime_secs;
                }
            }
            let mut payload_dirty = false;
            if matches!(page, Page::Payloads)
                && y_released
                && !menu_open
                && !nav_block
                && !nav_block_cleared
            {
                payload_dirty |= page_state.payloads.toggle_details();
            }
            if matches!(page, Page::Payloads)
                && !nav_block
                && !nav_block_cleared
                && !page_state.payloads.details_open
            {
                if !menu_open && a_released {
                    payload_dirty |= page_state.payloads.select_prev();
                }
                if !menu_open && b_released {
                    payload_dirty |= page_state.payloads.select_next();
                }
                if x_released {
                    payload_dirty |= page_state.payloads.run_selected(&palette, palette.black);
                }
            }
            if matches!(page, Page::Payloads) {
                if page_state.payloads.tick(&palette, palette.black) {
                    dirty.page = true;
                }
            }
            if payload_dirty {
                dirty.page = true;
            }
            let mut daemon_dirty = false;
            if matches!(page, Page::Daemon)
                && !nav_block
                && !nav_block_cleared
                && !menu_open
            {
                if a_released {
                    daemon_dirty |= page_state.daemon.select_prev();
                }
                if b_released {
                    daemon_dirty |= page_state.daemon.select_next();
                }
                if x_released {
                    daemon_dirty |= page_state.daemon.run_selected(&palette, palette.black);
                }
            }
            if daemon_dirty {
                dirty.page = true;
            }
            if dirty.any() {
                let bg_color = palette.black;
                let menu_bg = palette.black;
                let layout = Layout::compute(disp.bounding_box().size, &config, menu_open);
                let screen_h = layout.content_rect.size.height.max(layout.menu_rect.size.height);
                let title_style = MonoTextStyleBuilder::new()
                    .font(&FONT_8X13)
                    .text_color(config.header_text)
                    .background_color(config.header_bg)
                    .build();
                let body_style = MonoTextStyleBuilder::new()
                    .font(&FONT_6X9)
                    .text_color(palette.white)
                    .background_color(bg_color)
                    .build();

                // On first draw, clear the whole screen to remove any power-on artifacts.
                if !bg_drawn {
                    let _ = disp.clear(bg_color);
                }

                if dirty.menu {
                    if let Some(prev_layout) = last_layout {
                        if prev_menu_open {
                            let _ = prev_layout
                                .menu_clear_rect
                                .into_styled(PrimitiveStyle::with_fill(bg_color))
                                .draw(disp);
                        }
                    }
                    if menu_open {
                        let _ = layout
                            .menu_clear_rect
                            .into_styled(PrimitiveStyle::with_fill(menu_bg))
                            .draw(disp);
                        if let Some(divider) = layout.divider_rect {
                            let _ = divider
                                .into_styled(PrimitiveStyle::with_fill(palette.white))
                                .draw(disp);
                        }
                        let menu_base_y = 20;
                        let menu_item_h: i32 = 18;
                        for (idx, (_, item)) in MENU_ITEMS.iter().enumerate() {
                            draw_menu_item(
                                disp,
                                layout.menu_rect.top_left.x,
                                layout.menu_rect.size.width,
                                menu_base_y,
                                menu_item_h,
                                idx,
                                item,
                                idx == menu_selected,
                                menu_bg,
                                &palette,
                            );
                        }
                    }
                } else if dirty.divider {
                    if let Some(divider) = layout.divider_rect {
                        let _ = divider
                            .into_styled(PrimitiveStyle::with_fill(palette.white))
                            .draw(disp);
                    }
                } else if menu_changed && menu_open && bg_drawn {
                    if prev_menu_selected != usize::MAX && prev_menu_selected < MENU_ITEMS.len() {
                        let menu_base_y = 20;
                        let menu_item_h: i32 = 18;
                        draw_menu_item(
                            disp,
                            layout.menu_rect.top_left.x,
                            layout.menu_rect.size.width,
                            menu_base_y,
                            menu_item_h,
                            prev_menu_selected,
                            MENU_ITEMS[prev_menu_selected].1,
                            false,
                            menu_bg,
                            &palette,
                        );
                        draw_menu_item(
                            disp,
                            layout.menu_rect.top_left.x,
                            layout.menu_rect.size.width,
                            menu_base_y,
                            menu_item_h,
                            menu_selected,
                            MENU_ITEMS[menu_selected].1,
                            true,
                            menu_bg,
                            &palette,
                        );
                    }
                }

                if dirty.content {
                    let from_or_to_logs =
                        matches!(page, Page::Logs) || matches!(prev_page, Page::Logs);
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
                    if from_or_to_logs || !bg_drawn {
                        clear_rect.size.height = screen_h;
                    } else {
                        clear_rect.size.height = screen_h.min(140);
                    }
                    let _ = clear_rect
                        .into_styled(PrimitiveStyle::with_fill(bg_color))
                        .draw(disp);

                    page_state.reset();
                }

                // Clear the gap between menu and content regions if padding creates an untouched strip.
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

                match page {
                    Page::System => {
                        if dirty.page || dirty.content {
                            let uptime = Instant::now()
                                .checked_duration_since(start_instant)
                                .unwrap_or_default();
                            let uptime_text = format_hms(uptime);
                            let uptime_line: String<32> =
                                build_text_line("Uptime: ", uptime_text.as_str());
                            let cpu_line: String<32> = build_text_line("CPU: ", "n/a");
                            let temp_line: String<32> = match adc.read(&mut temp_channel).await {
                                Ok(raw) => {
                                    let temp_c = adc_temp_to_celsius(raw);
                                    let mut s: String<32> = String::new();
                                    let _ = write!(s, "Temp: {:.1}C", temp_c);
                                    s
                                }
                                Err(_) => build_text_line("Temp: ", "n/a"),
                            };
                            let heap_line: String<32> = build_text_line("Heap: ", "n/a");
                            let stack_line: String<32> = build_text_line("Stack: ", "n/a");
                            let psram_line: String<32> = psram_status_line();
                            let flash_line_ref = flash_line.as_str();

                            page_system::render(
                                disp,
                                line_x,
                                y_pos,
                                layout.content_width,
                                clear_w,
                                bg_color,
                                &config,
                                pins.ap_ssid,
                                page_system::SystemMetrics {
                                    uptime: uptime_line.as_str(),
                                    cpu: cpu_line.as_str(),
                                    temp: temp_line.as_str(),
                                    heap: heap_line.as_str(),
                                    stack: stack_line.as_str(),
                                    psram: psram_line.as_str(),
                                    flash: flash_line_ref,
                                },
                                &title_style,
                                &body_style,
                                &mut page_state.system,
                            );
                        }
                    }
                    Page::Payloads => {
                        if dirty.page || dirty.content {
                            page_payloads::render(
                                disp,
                                line_x,
                                y_pos,
                                clear_w,
                                layout.content_width,
                                layout.content_rect.size.height,
                                bg_color,
                                &config,
                                &title_style,
                                &body_style,
                                &mut page_state.payloads,
                            );
                        }
                    }
                    Page::Daemon => {
                        if dirty.page || dirty.content {
                            page_daemon::render(
                                disp,
                                line_x,
                                y_pos,
                                clear_w,
                                layout.content_width,
                                layout.content_rect.size.height,
                                bg_color,
                                &config,
                                &title_style,
                                &body_style,
                                &mut page_state.daemon,
                            );
                        }
                    }
                    Page::Logs => {
                        if dirty.page || dirty.content {
                            page_logs::render(
                                disp,
                                line_x,
                                y_pos,
                                clear_w,
                                layout.content_width,
                                layout.content_rect.size.height,
                                &config,
                                log_gen,
                                bg_color,
                                &title_style,
                                &body_style,
                                &mut page_state.logs,
                            );
                        }
                    }
                }

                prev_page = page;
                last_layout = Some(layout);
                bg_drawn = true;
            }
        } else if !display_fail_logged {
            log::warn!("display: ST7789 init failed; running buttons/LED only");
            display_fail_logged = true;
        }

        prev_a = a_pressed;
        prev_b = b_pressed;
        prev_x = x_pressed;
        prev_y = y_pressed;
        prev_menu_selected = menu_selected;
        prev_menu_open = menu_open;
    }
}

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_rp::Peri;
use embassy_rp::adc::{Adc, Channel, Config as AdcConfig};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{
    ADC, ADC_TEMP_SENSOR, PIN_12, PIN_13, PIN_14, PIN_15, PIN_16, PIN_17, PIN_18, PIN_19, PIN_20,
    PIN_26, PIN_27, PIN_28, SPI0,
};
use embassy_time::{Duration, Instant, Ticker};
use embedded_graphics::pixelcolor::{Rgb565, Rgb888};
use heapless::String;

use crate::log_buffer;

mod backend;
mod input;
mod page_common;
mod page_daemon;
mod page_logs;
mod page_system;
mod page_transfer;
mod pages;
mod renderer;
mod ui;

use backend::DisplayBackendPins;
use input::InputDebouncer;
use page_common::{build_text_line, format_hms};
use page_system::SystemMetrics;
use pages::{PageContext, PageId, PageInput, PageRegistry, PageRenderData};
use renderer::{PageRenderRequest, Renderer};
use ui::{UiState, apply_input};

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
    pub black: Rgb565,
    pub green: Rgb565,
}

impl DisplayPalette {
    pub fn new() -> Self {
        Self {
            white: Rgb565::from(Rgb888::new(0xFF, 0xFF, 0xFF)),
            yellow: Rgb565::from(Rgb888::new(0xF9, 0xDB, 0x6D)),
            teal: Rgb565::from(Rgb888::new(0x36, 0x82, 0x7F)),
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
        let status = if crate::psram_pool::is_available() {
            "ok"
        } else {
            "not detected"
        };
        build_text_line("PSRAM: ", status)
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

fn msc_reserved_bytes() -> usize {
    unsafe extern "C" {
        static __msc_start: u8;
        static __msc_end: u8;
    }
    let start = core::ptr::addr_of!(__msc_start) as usize;
    let end = core::ptr::addr_of!(__msc_end) as usize;
    end.saturating_sub(start)
}

fn persist_reserved_bytes() -> usize {
    unsafe extern "C" {
        static __persist_start: u8;
        static __persist_end: u8;
    }
    let start = core::ptr::addr_of!(__persist_start) as usize;
    let end = core::ptr::addr_of!(__persist_end) as usize;
    end.saturating_sub(start)
}

fn format_bytes_mb_one_decimal(bytes: usize) -> String<16> {
    const MIB: usize = 1024 * 1024;
    let total_tenths = (bytes * 10 + (MIB / 2)) / MIB;
    let whole = total_tenths / 10;
    let tenths = total_tenths % 10;
    let mut s: String<16> = String::new();
    let _ = write!(s, "{}.{}MB", whole, tenths);
    s
}

fn flash_status_line(total: usize, free: usize) -> String<48> {
    let used = total.saturating_sub(free);
    let pct_free = free
        .saturating_mul(100)
        .checked_div(total)
        .unwrap_or_default();
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

struct SystemMetricsSnapshot {
    uptime_line: String<32>,
    cpu_line: String<32>,
    temp_line: String<32>,
    heap_line: String<32>,
    stack_line: String<32>,
    psram_line: String<32>,
    flash_line: String<48>,
}

struct SystemMetricsTracker {
    start_instant: Instant,
    last_uptime_secs: u64,
    flash_line: String<48>,
}

impl SystemMetricsTracker {
    fn new(flash_total: usize, flash_free: usize) -> Self {
        Self {
            start_instant: Instant::now(),
            last_uptime_secs: 0,
            flash_line: flash_status_line(flash_total, flash_free),
        }
    }

    fn uptime_dirty(&mut self) -> bool {
        let uptime_secs = Instant::now()
            .checked_duration_since(self.start_instant)
            .unwrap_or_default()
            .as_secs();
        if uptime_secs != self.last_uptime_secs {
            self.last_uptime_secs = uptime_secs;
            return true;
        }
        false
    }

    async fn snapshot(
        &self,
        adc: &mut Adc<'_, embassy_rp::adc::Async>,
        temp_channel: &mut Channel<'_>,
    ) -> SystemMetricsSnapshot {
        let uptime = Instant::now()
            .checked_duration_since(self.start_instant)
            .unwrap_or_default();
        let uptime_text = format_hms(uptime);
        let uptime_line: String<32> = build_text_line("Uptime: ", uptime_text.as_str());
        let cpu_line: String<32> = build_text_line("CPU: ", "n/a");
        let temp_line: String<32> = match adc.read(temp_channel).await {
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
        let flash_line = self.flash_line.clone();

        SystemMetricsSnapshot {
            uptime_line,
            cpu_line,
            temp_line,
            heap_line,
            stack_line,
            psram_line,
            flash_line,
        }
    }
}

pub fn spawn(spawner: &Spawner, pins: DisplayPins<'static>) -> bool {
    crate::log_spawn(spawner, "display_task", display_task(pins))
}

fn apply_led(
    color: &LedColor,
    r: &mut Output<'static>,
    g: &mut Output<'static>,
    b: &mut Output<'static>,
) {
    // Active-low LED: low = on, high = off.
    if color.r {
        r.set_low()
    } else {
        r.set_high()
    };
    if color.g {
        g.set_low()
    } else {
        g.set_high()
    };
    if color.b {
        b.set_low()
    } else {
        b.set_high()
    };
}

#[embassy_executor::task]
async fn display_task(pins: DisplayPins<'static>) -> ! {
    log::info!("display task starting (buttons + basic screen)");

    let DisplayPins {
        adc,
        temp_sensor,
        spi,
        sck,
        mosi,
        cs,
        dc,
        backlight,
        btn_a,
        btn_b,
        btn_x,
        btn_y,
        led_r,
        led_g,
        led_b,
        ap_ssid,
    } = pins;

    // LED + buttons are always available even if display init fails.
    let mut led_r = Output::new(led_r, Level::High);
    let mut led_g = Output::new(led_g, Level::High);
    let mut led_b = Output::new(led_b, Level::High);

    let btn_a = Input::new(btn_a, Pull::Up);
    let btn_b = Input::new(btn_b, Pull::Up);
    let btn_x = Input::new(btn_x, Pull::Up);
    let btn_y = Input::new(btn_y, Pull::Up);

    // Backlight must stay driven; keep the pin alive for the whole task.
    let mut backlight = Output::new(backlight, Level::High);

    let mut adc = Adc::new(adc, AdcIrqs, AdcConfig::default());
    let mut temp_channel = Channel::new_temp_sensor(temp_sensor);

    // --- Display init (best-effort; keep running even if it fails) ---
    let mut display = backend::init(
        DisplayBackendPins {
            spi,
            sck,
            mosi,
            cs,
            dc,
        },
        &mut backlight,
    );

    let mut input = InputDebouncer::new();
    let mut pages = PageRegistry::new();
    let mut ui_state = UiState::new(pages.default_menu_index());
    let mut renderer = Renderer::new(
        DisplayConfig::default(),
        pages.page_for_index(ui_state.menu_selected),
    );
    let palette = renderer.palette();

    let mut color_idx = 0usize; // start off
    apply_led(&LED_COLORS[color_idx], &mut led_r, &mut led_g, &mut led_b);

    let mut ticker = Ticker::every(Duration::from_millis(50));
    let mut display_state_logged = false;
    let mut display_fail_logged = false;

    let flash_total = crate::device_config::FLASH_CAPACITY;
    let firmware_bytes = firmware_size_bytes();
    let reserved_bytes = msc_reserved_bytes().saturating_add(persist_reserved_bytes());
    let flash_used = firmware_bytes.saturating_add(reserved_bytes);
    let flash_free = flash_total.saturating_sub(flash_used);
    let mut metrics = SystemMetricsTracker::new(flash_total, flash_free);

    loop {
        ticker.next().await;

        let buttons = input.sample(&btn_a, &btn_b, &btn_x, &btn_y);
        if buttons.a.released {
            publish_script_button("A");
        }
        if buttons.b.released {
            publish_script_button("B");
        }
        if buttons.x.released {
            publish_script_button("X");
        }
        if buttons.y.released {
            publish_script_button("Y");
        }
        let ui_signals = apply_input(&mut ui_state, &buttons, pages.menu_len());
        let page = pages.page_for_index(ui_state.menu_selected);

        if buttons.y.released {
            color_idx = (color_idx + 1) % LED_COLORS.len();
            log::info!("Y press -> LED color {}", LED_COLORS[color_idx].name);
        }

        apply_led(&LED_COLORS[color_idx], &mut led_r, &mut led_g, &mut led_b);

        if let Some(ref mut disp) = display {
            if !display_state_logged {
                log::info!("display: ST7789 ready; entering render loop");
                display_state_logged = true;
            }

            let page_input = PageInput {
                a_released: buttons.a.released,
                b_released: buttons.b.released,
                x_released: buttons.x.released,
                menu_open: ui_state.menu_open,
                nav_blocked: ui_state.nav_blocked(),
                nav_block_cleared: ui_signals.nav_block_cleared,
            };
            let page_ctx = PageContext {
                palette: &palette,
                idle_bg: palette.black,
            };

            let mut page_dirty = pages.handle_input(page, &page_input, &page_ctx);
            page_dirty |= pages.on_tick(page, &page_ctx);

            if matches!(page, PageId::System) && metrics.uptime_dirty() {
                page_dirty = true;
            }

            let log_gen = if matches!(page, PageId::Logs) {
                log_buffer::generation()
            } else {
                pages.logs_last_gen()
            };
            let log_changed = matches!(page, PageId::Logs) && log_gen != pages.logs_last_gen();

            let plan = renderer.plan(disp, &ui_state, page, page_dirty, log_changed);
            if plan.should_draw() {
                let mut system_snapshot: Option<SystemMetricsSnapshot> = None;
                if matches!(page, PageId::System) && plan.needs_page_render() {
                    system_snapshot = Some(metrics.snapshot(&mut adc, &mut temp_channel).await);
                }

                let page_data = match page {
                    PageId::System => {
                        if let Some(ref snapshot) = system_snapshot {
                            PageRenderData::System {
                                ap_ssid,
                                metrics: SystemMetrics {
                                    uptime: snapshot.uptime_line.as_str(),
                                    cpu: snapshot.cpu_line.as_str(),
                                    temp: snapshot.temp_line.as_str(),
                                    heap: snapshot.heap_line.as_str(),
                                    stack: snapshot.stack_line.as_str(),
                                    psram: snapshot.psram_line.as_str(),
                                    flash: snapshot.flash_line.as_str(),
                                },
                            }
                        } else {
                            PageRenderData::None
                        }
                    }
                    PageId::Logs => PageRenderData::Logs { log_gen },
                    _ => PageRenderData::None,
                };

                let menu_items = pages.menu_items();
                renderer.render_with_plan(
                    disp,
                    &ui_state,
                    menu_items,
                    &mut pages,
                    plan,
                    PageRenderRequest {
                        id: page,
                        data: page_data,
                    },
                );
            }
        } else if !display_fail_logged {
            log::warn!("display: ST7789 init failed; running buttons/LED only");
            display_fail_logged = true;
        }
    }
}

fn publish_script_button(button: &str) {
    let mut event: String<{ crate::http::transfer::TRANSFER_TEXT_MAX }> = String::new();
    let _ = write!(
        event,
        "{{\"event_type\":\"script/event\",\"version\":1,\"event\":{{\"kind\":\"button\",\"button\":\"{}\",\"edge\":\"released\"}}}}",
        button
    );
    let _ = crate::http::transfer::queue_text(event);
}

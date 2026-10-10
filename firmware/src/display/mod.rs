//! Hardware ownership and orchestration for the Pico display.
use crate::{
    display_core::{
        Controller, DisplayConfig, MAX_LOG_ROWS, PageId, Renderer,
        input::{Button, InputDebouncer},
        model::Text,
    },
    log_buffer,
};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_rp::{
    Peri,
    adc::{Adc, Channel, Config as AdcConfig},
    bind_interrupts,
    gpio::{Input, Level, Output, Pull},
    peripherals::{
        ADC, ADC_TEMP_SENSOR, DMA_CH1, PIN_12, PIN_13, PIN_14, PIN_15, PIN_16, PIN_17, PIN_18,
        PIN_19, PIN_20, PIN_26, PIN_27, PIN_28, SPI0,
    },
};
use embassy_time::{Duration, Instant, Ticker};
use embedded_graphics::prelude::Dimensions;
use heapless::Vec;

mod backend;
mod diagnostics;
mod metrics;
mod services;

bind_interrupts!(struct AdcIrqs { ADC_IRQ_FIFO => embassy_rp::adc::InterruptHandler; });

// Pico Display 2.8 uses active-low RGB LED channels.
pub struct DisplayPins<'d> {
    pub adc: Peri<'d, ADC>,
    pub temp_sensor: Peri<'d, ADC_TEMP_SENSOR>,
    pub spi: Peri<'d, SPI0>,
    pub dma: Peri<'d, DMA_CH1>,
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
    let started = Instant::now();
    let DisplayPins {
        adc,
        temp_sensor,
        spi,
        dma,
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
    let mut inputs = Inputs {
        buttons: [
            Input::new(btn_a, Pull::Up),
            Input::new(btn_b, Pull::Up),
            Input::new(btn_x, Pull::Up),
            Input::new(btn_y, Pull::Up),
        ],
        leds: [
            Output::new(led_r, Level::High),
            Output::new(led_g, Level::High),
            Output::new(led_b, Level::High),
        ],
        debounce: InputDebouncer::default(),
        color_idx: 0,
    };
    // Keep the backlight output alive even if initialization fails.
    let mut backlight = Output::new(backlight, Level::High);
    let mut adc = Adc::new(adc, AdcIrqs, AdcConfig::default());
    let mut temperature = Channel::new_temp_sensor(temp_sensor);
    let mut display = backend::init(
        backend::DisplayBackendPins {
            spi,
            dma,
            sck,
            mosi,
            cs,
            dc,
        },
        &mut backlight,
    )
    .await;
    let mut controller = Controller::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut metrics = metrics::Metrics::new(started.as_millis());
    let mut diagnostics = diagnostics::Diagnostics::default();
    let mut logs: Vec<Text, MAX_LOG_ROWS> = Vec::new();
    let mut logs_generation = None;
    let mut ticker = Ticker::every(Duration::from_millis(50));

    let mut dirty = false;
    loop {
        ticker.next().await;
        dirty |= inputs.poll(&mut controller, &mut diagnostics, display.is_some());
        let pairing = crate::ble_control::pairing_code();
        dirty |= metrics.snapshot.pairing_code != pairing;
        metrics.snapshot.pairing_code = pairing;
        let now_ms = Instant::now().as_millis();
        if let Some(display) = display.as_mut() {
            let page = controller.page();
            let layout_dirty =
                renderer.needs_layout(display.bounding_box(), &controller.navigation, page);
            if page == PageId::System {
                dirty |= metrics.update(now_ms, &mut adc, &mut temperature).await;
            }
            if page == PageId::Logs
                && (layout_dirty || logs_generation != Some(log_buffer::generation()))
            {
                logs_generation = Some(log_buffer::snapshot_tail(&mut logs));
                dirty = true;
            }
            if dirty || layout_dirty {
                let before = backend::spi_counters();
                let render_started = Instant::now();
                let frame = renderer.prepare(
                    display.bounding_box(),
                    &controller.navigation,
                    controller.view(services::presets(), ap_ssid, &metrics.snapshot, &logs),
                );
                dirty = false;
                let result = {
                    // select borrows the pinned flush; timer wins never cancel DMA.
                    // Models can advance while the owned frame retains its old text.
                    let mut flush = core::pin::pin!(display.flush(&frame));
                    loop {
                        match select(flush.as_mut(), ticker.next()).await {
                            Either::First(result) => break result,
                            Either::Second(()) => {
                                dirty |= inputs.poll(&mut controller, &mut diagnostics, true);
                            }
                        }
                    }
                };
                let elapsed = Instant::now().duration_since(render_started).as_micros();
                let after = backend::spi_counters();
                diagnostics.render(
                    elapsed,
                    after.0.wrapping_sub(before.0),
                    after.1.wrapping_sub(before.1),
                    backend::max_raster_us(),
                    result.as_ref().copied().unwrap_or(false),
                );
                match result {
                    Ok(_) => {
                        renderer.commit(frame);
                        if let Some(code) = metrics.snapshot.pairing_code {
                            crate::ble_control::mark_displayed(code);
                        }
                    }
                    Err(error) => {
                        renderer.invalidate();
                        log::warn!("display draw failed: {:?}", error);
                    }
                }
            } else {
                diagnostics.idle();
            }
        }
        diagnostics.report(Instant::now().as_millis());
    }
}

/// Input servicing is shared by idle ticks and ticks during an awaited DMA flush.
struct Inputs {
    buttons: [Input<'static>; 4],
    leds: [Output<'static>; 3],
    debounce: InputDebouncer,
    color_idx: usize,
}

impl Inputs {
    fn poll(
        &mut self,
        controller: &mut Controller,
        diagnostics: &mut diagnostics::Diagnostics,
        display_available: bool,
    ) -> bool {
        let sampled_at = Instant::now();
        diagnostics.sample(sampled_at.as_micros());
        let events = self
            .debounce
            .sample(core::array::from_fn(|index| self.buttons[index].is_low()));
        if crate::ble_control::pairing_code().is_some() {
            controller.navigation = crate::display_core::input::Navigation::new(
                PageId::ALL
                    .iter()
                    .position(|page| *page == PageId::System)
                    .unwrap_or_default(),
            );
            if events[Button::Y].just_pressed {
                crate::usb::hid::stop_device();
                crate::ble_control::DECISION.signal(false);
            } else if display_available
                && crate::ble_control::pairing_displayed()
                && events[Button::X].just_pressed
            {
                crate::ble_control::DECISION.signal(true);
            }
            return true;
        }
        let snapshot = services::snapshot(sampled_at.as_millis());
        let reserved = snapshot.keyboard.reserved() || crate::usb::bootstrap::active();
        let mut dirty = controller.update(snapshot, services::take_completion());
        dirty |= services::refresh_installation(controller);
        let routing = controller.route(events, reserved);
        if routing.stop {
            crate::usb::hid::stop_device();
            diagnostics.stop(Instant::now().duration_since(sampled_at).as_micros());
        }
        if routing.cycle_led {
            self.color_idx = (self.color_idx + 1) % LED_COLORS.len();
            let color = &LED_COLORS[self.color_idx];
            log::info!("Y press -> LED color {}", color.name);
            let [r, g, b] = &mut self.leds;
            apply_led(color, r, g, b);
        }
        // A failed panel must not disable device-wide Stop or script events.
        if display_available {
            dirty |= routing.page != Default::default();
            if let Some(intent) = controller.handle_input(routing.page, services::presets()) {
                services::execute(controller, intent);
            }
        }
        dirty
    }
}

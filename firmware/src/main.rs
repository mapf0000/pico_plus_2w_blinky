#![no_std]
#![no_main]
#![recursion_limit = "256"]

mod bootstrap_core;

// ===== Imports =====

// use core::sync::atomic::Ordering; // no longer used in this module

use cyw43_pio::PioSpi; // RM2 divider is used directly via RM2_CLOCK_DIVIDER
use cyw43_pio::RM2_CLOCK_DIVIDER;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::dma;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, PIO0, USB};
use embassy_rp::pio::Pio;
// USB classes are handled in `crate::usb` now
use static_cell::StaticCell;
// HID report descriptors handled in `crate::usb` now

use {defmt_rtt as _, panic_probe as _};

// ===== Interrupt bindings =====

bind_interrupts!(pub struct Irqs {
    // USB controller interrupt for the logger
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    // PIO interrupt for CYW43 PIO-SPI
    PIO0_IRQ_0  => embassy_rp::pio::InterruptHandler<PIO0>;
    // Separate DMA channels for CYW43 PIO-SPI and display SPI0.
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH0>, embassy_rp::dma::InterruptHandler<DMA_CH1>;
});

// ===== Small utilities =====

#[inline]
pub fn log_spawn<S>(
    spawner: &Spawner,
    name: &str,
    token: Result<embassy_executor::SpawnToken<S>, embassy_executor::SpawnError>,
) -> bool {
    match token {
        Ok(token) => {
            spawner.spawn(token);
            true
        }
        Err(e) => {
            log::error!("spawn {} failed: {:?}", name, e);
            false
        }
    }
}

// ===== Tasks =====

/// Drives low-level CYW43 events
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>>,
) -> ! {
    runner.run().await
}

// ===== Orchestration =====

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // 0) Peripherals
    let p = embassy_rp::init(Default::default());

    // Recover automatically if a cooperative task stalls the executor. The
    // retained stage is reported over CDC on the next boot.
    let _ = health::spawn(&spawner, p.WATCHDOG);

    // 1) Early USB for logs (CDC) + HID (kept running after)
    // Prepare USB supervisor: start USB for the host connection
    crate::usb::usb_supervisor::init(spawner);

    // 2) Runtime config + (optional) PSRAM + flash persistence
    crate::device_config::init().await;

    // Bring up the Pico Display 2.8 (ST7789 + buttons + RGB LED).
    let display_pins = display::DisplayPins {
        adc: p.ADC,
        temp_sensor: p.ADC_TEMP_SENSOR,
        spi: p.SPI0,
        dma: p.DMA_CH1,
        sck: p.PIN_18,
        mosi: p.PIN_19,
        cs: p.PIN_17,
        dc: p.PIN_16,
        backlight: p.PIN_20,
        btn_a: p.PIN_12,
        btn_b: p.PIN_13,
        btn_x: p.PIN_14,
        btn_y: p.PIN_15,
        led_r: p.PIN_26,
        led_g: p.PIN_27,
        led_b: p.PIN_28,
        ap_ssid: "Bluetooth",
    };
    let _ = display::spawn(&spawner, display_pins);

    #[cfg(feature = "psram")]
    {
        psram_pool::init(p.QMI_CS1, p.PIN_47).await;
    }

    let flash_drv = embassy_rp::flash::Flash::<
        _,
        embassy_rp::flash::Blocking,
        { crate::device_config::FLASH_CAPACITY },
    >::new_blocking(p.FLASH);
    crate::device_config::set_flash_driver(flash_drv).await;

    // --- CYW43 bring-up via PIO-SPI ---
    let fw = cyw43::aligned_bytes!("../cyw43-firmware/43439A0.bin");
    let nvram = cyw43::aligned_bytes!("../cyw43-firmware/nvram_rp2040.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);

    let mut pio = Pio::new(p.PIO0, Irqs);
    log::info!("pio: initializing CYW43 PIO-SPI interface");
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER, // important for RM2-based Pico Plus 2 W
        pio.irq0,
        cs,
        p.PIN_24, // bidirectional data
        p.PIN_29, // clock
        dma::Channel::new(p.DMA_CH0, Irqs),
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    log::info!("cyw43: loading firmware and bringing up chip");
    let (_net_device, bt_driver, _control, cyw_runner) = cyw43::new_with_bluetooth(
        state,
        pwr,
        spi,
        fw,
        cyw43::aligned_bytes!("../cyw43-firmware/43439A0_btfw.bin"),
        nvram,
    )
    .await;
    log::info!("cyw43: init complete; spawning runner");
    let _ = log_spawn(&spawner, "cyw43_task", cyw43_task(cyw_runner));

    {
        let mut address = [0; 6];
        RoscRng.fill_bytes(&mut address);
        // Static random address: two high bits set, random portion not all 0/1.
        address[5] |= 0xc0;
        address[0] = (address[0] & 0xfe) | 0x02;
        let _ = log_spawn(&spawner, "ble_task", ble::task(bt_driver, address));
    }

    // Park forever; tasks run forever.
    core::future::pending::<()>().await;
}

// ===== Submodules (kept declared; implementations live in their own files) =====

mod ble;
mod ble_control;
mod capabilities;
mod device_config;
mod display;
mod display_core;
mod health;
mod log_buffer;
#[cfg(feature = "psram")]
mod psram_pool;
mod usb;
// mod usb_ctrl; // disabled: control CDC removed to keep only keyboard + logging

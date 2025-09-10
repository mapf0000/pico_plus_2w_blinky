#![no_std]
#![no_main]

use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER}; // RM2 divider recommended on Pico Plus 2 W
use embassy_executor::Spawner;
use embassy_net::{self as net, Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::Pio;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_time::Timer;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    // USB controller interrupt for the logger
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    // PIO interrupt for CYW43 PIO-SPI
    PIO0_IRQ_0  => embassy_rp::pio::InterruptHandler<PIO0>;
});

/// USB logger task (sets global logger and runs forever)
#[embassy_executor::task]
async fn usb_logger_task(driver: UsbDriver<'static, USB>) {
    // 1024-byte buffer, log level = Info.
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

/// Drives low-level CYW43 events
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

/// Drives the network stack
#[embassy_executor::task]
async fn net_task(mut runner: net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Start USB logging first so we see everything else.
    let usb_driver = UsbDriver::new(p.USB, Irqs);
    spawner.spawn(usb_logger_task(usb_driver)).unwrap();

    let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER, // important for RM2-based Pico Plus 2 W
        pio.irq0,
        cs,
        p.PIN_24, // MOSI
        p.PIN_29, // MISO
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, cyw_runner) = cyw43::new(state, pwr, spi, fw).await;
    spawner.spawn(cyw43_task(cyw_runner)).unwrap();

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // --- Embassy net stack (DHCPv4) ---
    static NET_RES: StaticCell<StackResources<3>> = StaticCell::new();
    let mut rng = RoscRng;
    let seed = rng.next_u64();

    let cfg = Config::dhcpv4(Default::default());
    let (stack, net_runner) = net::new(net_device, cfg, NET_RES.init(StackResources::new()), seed);
    spawner.spawn(net_task(net_runner)).unwrap();

    // --- Join your WLAN ---
    const SSID: &str = "Fledermausland";
    const PASS: &str = "Wir!123Koennen?Hier!Nicht?Halten!456";

    loop {
        match control
            .join(SSID, cyw43::JoinOptions::new(PASS.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(e) => {
                log::warn!("join failed (status={:?}), retrying in 1s", e.status);
                Timer::after_secs(1).await;
            }
        }
    }
    log::info!("WiFi associated");

    // Wait for DHCP/stack to be usable
    stack.wait_config_up().await;
    log::info!("Network is up: {:?}", stack.config_v4());

    // ...use sockets via `stack` (TCP/UDP/DNS). Your app logic goes here...
}

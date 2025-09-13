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
    // ADC interrupt (used by ADC driver)
    ADC_IRQ_FIFO => embassy_rp::adc::InterruptHandler;
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
    // From here on, logs should be visible over USB.
    log::info!("usb: logger task spawned");

    let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

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
        p.PIN_24, // MOSI
        p.PIN_29, // MISO
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    log::info!("cyw43: loading firmware and bringing up chip");
    let (net_device, mut control, cyw_runner) = cyw43::new(state, pwr, spi, fw).await;
    log::info!("cyw43: init complete; spawning runner");
    spawner.spawn(cyw43_task(cyw_runner)).unwrap();

    log::info!("cyw43: applying CLM/regulatory data");
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;
    log::info!("cyw43: power management set to PowerSave");

    // --- Embassy net stack (DHCPv4) ---
    static NET_RES: StaticCell<StackResources<3>> = StaticCell::new();
    let mut rng = RoscRng;
    let seed = rng.next_u64();
    log::info!("net: rng seeded with 0x{:08x}{:08x}", (seed >> 32) as u32, seed as u32);

    let cfg = Config::dhcpv4(Default::default());
    let (stack_val, net_runner) =
        net::new(net_device, cfg, NET_RES.init(StackResources::new()), seed);
    static NET_STACK: StaticCell<net::Stack<'static>> = StaticCell::new();
    let stack = NET_STACK.init(stack_val);
    spawner.spawn(net_task(net_runner)).unwrap();
    log::info!("net: stack runner spawned (DHCPv4)");

    // --- Join your WLAN ---
    // const SSID: &str = "Fledermausland";
    // const PASS: &str = "Wir!123Koennen?Hier!Nicht?Halten!456";
    const SSID: &str = "MagentaWLAN-MCMT";
    const PASS: &str = "31828370613283878587";

    log::info!("wifi: connecting to SSID '{}'", SSID);
    let mut attempt: u32 = 1;
    loop {
        match control
            .join(SSID, cyw43::JoinOptions::new(PASS.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(e) => {
                log::warn!("wifi: join attempt {} failed (status={:?}), retrying in 1s", attempt, e.status);
                attempt = attempt.saturating_add(1);
                Timer::after_secs(1).await;
            }
        }
    }
    log::info!("wifi: associated to '{}'", SSID);

    // Wait for DHCP/stack to be usable
    log::info!("net: waiting for DHCP/stack config");
    stack.wait_config_up().await;
    log::info!("net: up: {:?}", stack.config_v4());

    // --- Temperature sampling + HTTP exposure ---
    // Shared state for latest temperature reading
    static SHARED_CELL: StaticCell<temp::Shared> = StaticCell::new();
    let shared = SHARED_CELL.init(temp::Shared::new());

    // Create ADC sampling task
    spawner
        .spawn(temp::sampling_task(p.ADC, p.ADC_TEMP_SENSOR, shared))
        .unwrap();
    log::info!("temp: sampling task spawned");


    // Spawn a tiny HTTP server exposing /temp and /metrics
    spawner.spawn(http::server_task(stack, shared)).unwrap();
    log::info!("http: server task spawned (port 80)");

    // Main can park; tasks run forever.
    core::future::pending::<()>().await;
}

mod http;
mod temp;

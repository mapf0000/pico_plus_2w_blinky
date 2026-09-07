#![no_std]
#![no_main]
#![recursion_limit = "256"]

// ===== Imports =====

// use core::sync::atomic::Ordering; // no longer used in this module

use cyw43_pio::PioSpi; // RM2 divider is used directly via RM2_CLOCK_DIVIDER
use cyw43_pio::RM2_CLOCK_DIVIDER;
use embassy_executor::Spawner;
use embassy_net::{self as net, Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::dma;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::Pio;
// USB classes are handled in `crate::usb` now
use static_cell::StaticCell;
// HID report descriptors handled in `crate::usb` now

use crate::http::{spawn_http_server_pool, spawn_websocket_server};

use {defmt_rtt as _, panic_probe as _};

// ===== Interrupt bindings =====

bind_interrupts!(pub struct Irqs {
    // USB controller interrupt for the logger
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    // PIO interrupt for CYW43 PIO-SPI
    PIO0_IRQ_0  => embassy_rp::pio::InterruptHandler<PIO0>;
    // DMA channel used by the CYW43 PIO-SPI transport.
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH0>;
});

// ===== Module-local constants (small & auditable) =====

const AP_SSID: &str = "PicoEndpoint";
const AP_PASS: &str = "pico12345"; // WPA2: 8+ chars
const AP_CHANNEL: u8 = 6;

// USB config moved under `usb::task`

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

/// Seed RNG and log the value; returns the seed.
fn seed_rng() -> u64 {
    let mut rng = RoscRng;
    let seed = rng.next_u64();
    log::info!(
        "net: rng seeded with 0x{:08x}{:08x}",
        (seed >> 32) as u32,
        seed as u32
    );
    seed
}

/// Initialize the network stack with a static IPv4 config for AP mode.
fn init_net_stack(
    net_device: cyw43::NetDriver<'static>,
    seed: u64,
) -> (
    &'static net::Stack<'static>,
    net::Runner<'static, cyw43::NetDriver<'static>>,
) {
    // Increase socket pool: accommodate DHCP server + HTTP listener + multiple connections.
    // "3" can starve the HTTP server; 16 leaves headroom without being excessive.
    static NET_RES: StaticCell<StackResources<16>> = StaticCell::new();
    static NET_STACK: StaticCell<net::Stack<'static>> = StaticCell::new();

    // Static AP gateway: 192.168.4.1/24
    let cfg = Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(embassy_net::Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: None,
        dns_servers: Default::default(),
    });

    let (stack_val, runner) = net::new(net_device, cfg, NET_RES.init(StackResources::new()), seed);
    let stack = NET_STACK.init(stack_val);
    log::info!("net: stack runner prepared (static IPv4)");
    (stack, runner)
}

/// Start WPA2 AP with constants above.
async fn start_access_point(control: &mut cyw43::Control<'static>) {
    log::info!("wifi: starting AP '{}' on channel {}", AP_SSID, AP_CHANNEL);
    control.start_ap_wpa2(AP_SSID, AP_PASS, AP_CHANNEL).await;
    log::info!("wifi: AP started; clients can connect to '{}'", AP_SSID);
}

/// Spawn DHCP server.
fn spawn_dhcp(spawner: &Spawner, stack: &'static net::Stack<'static>) -> bool {
    let ok = log_spawn(spawner, "dhcp::server_task", dhcp::server_task(stack));
    if ok {
        log::info!("dhcp: server task spawned (port 67)");
    }
    ok
}

/// Spawn tiny HTTP server for the USB trigger endpoint.
fn spawn_http(spawner: &Spawner, stack: &'static net::Stack<'static>) -> bool {
    let transfer_pump_ok = crate::http::transfer::spawn(spawner);
    let websocket_ok = spawn_websocket_server(spawner, *stack);
    spawn_http_server_pool(spawner, *stack);
    log::info!("http: asset workers on port 80; WebSocket handoff acceptors on port 81");
    transfer_pump_ok && websocket_ok
}

// ===== Tasks =====

/// Drives low-level CYW43 events
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>>,
) -> ! {
    runner.run().await
}

/// Drives the network stack
#[embassy_executor::task]
async fn net_task(mut runner: net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
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
    // Prepare USB supervisor: start USB on demand via WS command
    crate::usb::usb_supervisor::init(spawner);

    // 2) Runtime config + (optional) PSRAM + flash persistence
    crate::device_config::init().await;

    // Bring up the Pico Display 2.8 (ST7789 + buttons + RGB LED).
    let display_pins = display::DisplayPins {
        adc: p.ADC,
        temp_sensor: p.ADC_TEMP_SENSOR,
        spi: p.SPI0,
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
        ap_ssid: AP_SSID,
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
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");
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
    let (net_device, mut control, cyw_runner) = cyw43::new(state, pwr, spi, fw, nvram).await;
    log::info!("cyw43: init complete; spawning runner");
    let _ = log_spawn(&spawner, "cyw43_task", cyw43_task(cyw_runner));

    log::info!("cyw43: applying CLM/regulatory data");
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::None)
        .await;
    log::info!("cyw43: power management set to None (AP mode)");

    // --- Embassy net stack (Static IPv4 for AP mode) ---
    let seed = seed_rng();
    let (stack, net_runner) = init_net_stack(net_device, seed);
    let _ = log_spawn(&spawner, "net_task", net_task(net_runner));
    log::info!("net: stack runner spawned (static IPv4)");

    // --- Bring up a WPA2-protected Access Point ---
    start_access_point(&mut control).await;

    // Wait for DHCP/stack to be usable
    log::info!("net: waiting for stack config");
    stack.wait_config_up().await;
    log::info!("net: up: {:?}", stack.config_v4());

    // Start DHCP server for AP clients
    let _ = spawn_dhcp(&spawner, stack);

    // Start a tiny HTTP server to receive the USB trigger (don’t block on config)
    let _ = spawn_http(&spawner, stack);

    // USB will be started on demand; HTTP/WebSocket endpoint commands control it.

    // Park forever; tasks run forever.
    core::future::pending::<()>().await;
}

// ===== Submodules (kept declared; implementations live in their own files) =====

mod capabilities;
mod device_config;
mod dhcp;
mod display;
mod health;
mod host;
mod http;
mod log_buffer;
#[cfg(feature = "psram")]
mod psram_pool;
mod usb;
// mod usb_ctrl; // disabled: control CDC removed to keep only keyboard + logging

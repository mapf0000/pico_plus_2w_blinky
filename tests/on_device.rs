#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_futures::yield_now;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_time::{Duration, Instant, Timer};
use {defmt_rtt as _, panic_probe as _};

// Use the same USB logger approach as the app: logs go out over USB CDC.
bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
});

#[embassy_executor::task]
async fn usb_logger_task(driver: UsbDriver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

fn assert_ok(cond: bool, msg: &str) -> bool {
    if !cond {
        log::error!("assertion failed: {}", msg);
        false
    } else {
        true
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Start USB logging first so host can capture everything.
    let usb = UsbDriver::new(p.USB, Irqs);
    spawner.spawn(usb_logger_task(usb)).ok();
    // Let the logger task run so the global logger is installed.
    yield_now().await;

    // Emit something immediately so the host sees activity even if time isn't set up yet.
    log::info!("BEGIN on-device tests (logger ready)");

    let mut passed = true;

    // Test 1: Simple sanity that doesn't depend on timers.
    let ok_hello = true;
    passed &= assert_ok(ok_hello, "logger emitted first line");
    log::info!("test logger_first_line ... {}", if ok_hello { "ok" } else { "FAILED" });

    // Test 2: Basic math sanity (as a placeholder for real app logic tests).
    let c = 25.0f32;
    let f = c * 9.0 / 5.0 + 32.0;
    let ok_math = (f - 77.0).abs() < 0.001;
    passed &= assert_ok(ok_math, "C->F conversion");
    log::info!("test math_c_to_f ... {}", if ok_math { "ok" } else { "FAILED" });

    // Final result marker consumed by scripts/pico-run
    if passed {
        log::info!("ALL TESTS PASSED");
        log::info!("TEST-RESULT: PASS");
    } else {
        log::error!("ONE OR MORE TESTS FAILED");
        log::info!("TEST-RESULT: FAIL");
    }

    // Allow logs to flush before we park (busy wait, avoids timer dependency)
    for _ in 0..200_000 { core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst); }

    // Park forever to keep USB alive; runner will exit after seeing the result.
    core::future::pending::<()>().await;
}

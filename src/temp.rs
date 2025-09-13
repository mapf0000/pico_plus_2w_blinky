use core::fmt::Write;

use embassy_rp::peripherals::{ADC, ADC_TEMP_SENSOR};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};

// Shared state exposed to other tasks.
pub struct Shared {
    inner: Mutex<CriticalSectionRawMutex, Reading>,
}

impl Shared {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(Reading::invalid()),
        }
    }

    pub async fn get(&self) -> Reading {
        // Lock briefly to copy the latest reading.
        let guard = self.inner.lock().await;
        *guard
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Reading {
    pub celsius: f32,
    pub fahrenheit: f32,
    pub uptime_ms: u64,
    pub valid: bool,
}

impl Reading {
    pub const fn invalid() -> Self {
        Self { celsius: 0.0, fahrenheit: 0.0, uptime_ms: 0, valid: false }
    }
}

// Conversion constants from RP2040 datasheet; allow overriding if needed later.
const VREF: f32 = 3.3;
const ADC_MAX: f32 = 4095.0; // 12-bit ADC
const V_AT_27C: f32 = 0.706; // Volts at 27°C
const SLOPE_V_PER_C: f32 = 0.001721; // Volts per °C

pub fn raw_to_celsius(raw: u16) -> f32 {
    let v = (raw as f32) * VREF / ADC_MAX;
    27.0 - ((v - V_AT_27C) / SLOPE_V_PER_C)
}

pub fn c_to_f(c: f32) -> f32 {
    c * 9.0 / 5.0 + 32.0
}

// Simple exponential moving average filter for noise reduction.
fn ema(prev: f32, new: f32, alpha: f32) -> f32 {
    prev * (1.0 - alpha) + new * alpha
}

#[embassy_executor::task]
pub async fn sampling_task(
    adc_periph: embassy_rp::Peri<'static, ADC>,
    ts_periph: embassy_rp::Peri<'static, ADC_TEMP_SENSOR>,
    shared: &'static Shared,
) {
    // Construct ADC once inside the task (async mode).
    let mut adc = embassy_rp::adc::Adc::new(adc_periph, crate::Irqs, Default::default());
    // Internal temperature sensor channel
    let mut ch = embassy_rp::adc::Channel::new_temp_sensor(ts_periph);

    // Sampling config
    let period = Duration::from_millis(1000);
    let alpha = 0.2; // EMA smoothing factor

    log::info!("temp: sampling started (period={}ms, alpha={:.2})", period.as_millis(), alpha);

    let mut filtered_c = 0.0f32;
    let mut uptime_ms: u64 = 0;

    loop {
        // Async read; returns raw 12-bit sample.
        let raw: u16 = match adc.read(&mut ch).await {
            Ok(v) => v,
            Err(_) => {
                // Mark invalid and try again next tick
                let mut lock = shared.inner.lock().await;
                lock.valid = false;
                log::warn!("temp: adc read failed; marking invalid");
                Timer::after(period).await;
                uptime_ms = uptime_ms.saturating_add(period.as_millis() as u64);
                continue;
            }
        };
        let c = raw_to_celsius(raw);
        filtered_c = if uptime_ms == 0 { c } else { ema(filtered_c, c, alpha) };
        let f = c_to_f(filtered_c);

        // Update shared state
        {
            let mut lock = shared.inner.lock().await;
            lock.celsius = filtered_c;
            lock.fahrenheit = f;
            lock.uptime_ms = uptime_ms;
            lock.valid = true;
        }
        log::debug!(
            "temp: updated c={:.2}C f={:.2}F uptime={}ms (raw={})",
            filtered_c,
            f,
            uptime_ms,
            raw
        );

        Timer::after(period).await;
        uptime_ms = uptime_ms.saturating_add(period.as_millis() as u64);
    }
}

// Helper for building small JSON without allocations on the heap.
pub fn write_json(buf: &mut impl Write, r: Reading) {
    let _ = write!(
        buf,
        "{{\"c\":{:.2},\"f\":{:.2},\"uptime_ms\":{},\"valid\":{}}}",
        r.celsius,
        r.fahrenheit,
        r.uptime_ms,
        if r.valid { "true" } else { "false" }
    );
}

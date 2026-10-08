//! Board metrics. Linker addresses are sampled once; ADC reads remain asynchronous.

use crate::display_core::SystemSnapshot;
use embassy_rp::adc::{Adc, Async, Channel};

pub struct Metrics {
    started_ms: u64,
    last_sample_second: Option<u64>,
    pub snapshot: SystemSnapshot,
}

impl Metrics {
    pub fn new(started_ms: u64) -> Self {
        unsafe extern "C" {
            static _ram_start: u8;
            static _ram_end: u8;
            static __euninit: u8;
            static __start_block_addr: u8;
            static __end_block_addr: u8;
            static __msc_start: u8;
            static __msc_end: u8;
            static __persist_start: u8;
            static __persist_end: u8;
        }
        // Only addresses are used; no linker-delimited memory is dereferenced.
        let start = core::ptr::addr_of!(_ram_start) as usize;
        let sram_total = (core::ptr::addr_of!(_ram_end) as usize).saturating_sub(start);
        let sram_used = (core::ptr::addr_of!(__euninit) as usize)
            .saturating_sub(start)
            .min(sram_total);
        let firmware = (core::ptr::addr_of!(__end_block_addr) as usize)
            .saturating_sub(core::ptr::addr_of!(__start_block_addr) as usize);
        let msc = (core::ptr::addr_of!(__msc_end) as usize)
            .saturating_sub(core::ptr::addr_of!(__msc_start) as usize);
        let persistent = (core::ptr::addr_of!(__persist_end) as usize)
            .saturating_sub(core::ptr::addr_of!(__persist_start) as usize);
        let total = crate::device_config::FLASH_CAPACITY;
        Self {
            started_ms,
            last_sample_second: None,
            snapshot: SystemSnapshot {
                sram_static_bytes: sram_used,
                sram_total_bytes: sram_total,
                flash_total_bytes: total,
                flash_free_bytes: total
                    .saturating_sub(firmware.saturating_add(msc).saturating_add(persistent)),
                ..Default::default()
            },
        }
    }

    pub async fn update(
        &mut self,
        now_ms: u64,
        adc: &mut Adc<'_, Async>,
        temperature: &mut Channel<'_>,
    ) -> bool {
        let second = now_ms.saturating_sub(self.started_ms) / 1_000;
        if self.last_sample_second == Some(second) {
            return false;
        }
        self.last_sample_second = Some(second);
        self.snapshot.uptime_secs = second;
        self.snapshot.cpu_mhz = embassy_rp::clocks::clk_sys_freq() / 1_000_000;
        self.snapshot.temperature_c = adc.read(temperature).await.ok().map(|raw| {
            let voltage = raw as f32 * 3.3 / 4096.0;
            27.0 - (voltage - 0.706) / 0.001721
        });
        #[cfg(feature = "psram")]
        {
            self.snapshot.psram_status = if crate::psram_pool::is_available() {
                "ok"
            } else {
                "not detected"
            };
        }
        #[cfg(not(feature = "psram"))]
        {
            self.snapshot.psram_status = "disabled";
        }
        true
    }
}

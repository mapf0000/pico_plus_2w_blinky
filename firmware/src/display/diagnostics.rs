//! Bounded timing summaries; no page text, credentials, or file data is recorded.

#[derive(Default)]
pub struct Diagnostics {
    last_sample_us: Option<u64>,
    next_summary_ms: u64,
    frames: u32,
    idle_ticks: u32,
    missed_ticks: u32,
    max_sample_gap_us: u64,
    max_frame_us: u64,
    max_raster_us: u32,
    max_spi_bytes: u32,
    max_spi_writes: u32,
    stops: u32,
    max_stop_dispatch_us: u64,
}

impl Diagnostics {
    pub fn sample(&mut self, now_us: u64) {
        if let Some(previous) = self.last_sample_us.replace(now_us) {
            let gap = now_us.saturating_sub(previous);
            self.max_sample_gap_us = self.max_sample_gap_us.max(gap);
            self.missed_ticks = self
                .missed_ticks
                .saturating_add((gap / 50_000).saturating_sub(1).min(u32::MAX as u64) as u32);
        }
    }

    pub fn render(&mut self, elapsed_us: u64, bytes: u32, writes: u32, raster_us: u32, drew: bool) {
        self.max_frame_us = self.max_frame_us.max(elapsed_us);
        self.max_raster_us = self.max_raster_us.max(raster_us);
        self.max_spi_bytes = self.max_spi_bytes.max(bytes);
        self.max_spi_writes = self.max_spi_writes.max(writes);
        if drew {
            self.frames = self.frames.saturating_add(1);
        } else {
            self.idle();
        }
    }

    pub fn idle(&mut self) {
        self.idle_ticks = self.idle_ticks.saturating_add(1);
    }

    pub fn stop(&mut self, elapsed_us: u64) {
        self.stops = self.stops.saturating_add(1);
        self.max_stop_dispatch_us = self.max_stop_dispatch_us.max(elapsed_us);
    }

    pub fn report(&mut self, now_ms: u64) {
        if self.next_summary_ms == 0 {
            self.next_summary_ms = now_ms.saturating_add(30_000);
        } else if now_ms >= self.next_summary_ms {
            log::info!(
                "display metrics: frames={} idle={} missed_ticks={} max_sample_gap_us={} max_frame_us={} max_raster_us={} max_spi_bytes={} max_spi_writes={} stops={} max_stop_dispatch_us={}",
                self.frames,
                self.idle_ticks,
                self.missed_ticks,
                self.max_sample_gap_us,
                self.max_frame_us,
                self.max_raster_us,
                self.max_spi_bytes,
                self.max_spi_writes,
                self.stops,
                self.max_stop_dispatch_us
            );
            self.next_summary_ms = now_ms.saturating_add(30_000);
        }
    }
}

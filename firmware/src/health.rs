use embassy_executor::Spawner;
use embassy_rp::Peri;
use embassy_rp::peripherals::WATCHDOG;
use embassy_rp::watchdog::{ResetReason, Watchdog};
use embassy_time::{Duration, Timer};

// Scratch 0-5 participate in RP2350 boot-ROM reboot handoff. Keep diagnostic
// state in the two high scratch registers so the ROM does not rewrite it.
const SCRATCH_MAGIC_INDEX: usize = 6;
const SCRATCH_STAGE_INDEX: usize = 7;
const SCRATCH_MAGIC: u32 = 0x5049_434f; // "PICO"
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(6);
const WATCHDOG_FEED_INTERVAL: Duration = Duration::from_secs(1);

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Boot = 0,
    HttpIndex = 1,
    HttpJavascript = 2,
    HttpStylesheet = 3,
    HttpWasm = 4,
    HttpIndexedDb = 5,
    HttpHealth = 6,
    WebSocketUpgrade = 7,
    WebSocketHello = 8,
    WebSocketActive = 9,
}

impl Stage {
    fn from_raw(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Boot,
            1 => Self::HttpIndex,
            2 => Self::HttpJavascript,
            3 => Self::HttpStylesheet,
            4 => Self::HttpWasm,
            5 => Self::HttpIndexedDb,
            6 => Self::HttpHealth,
            7 => Self::WebSocketUpgrade,
            8 => Self::WebSocketHello,
            9 => Self::WebSocketActive,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::HttpIndex => "http:index",
            Self::HttpJavascript => "http:javascript",
            Self::HttpStylesheet => "http:stylesheet",
            Self::HttpWasm => "http:wasm",
            Self::HttpIndexedDb => "http:indexed-db",
            Self::HttpHealth => "http:health",
            Self::WebSocketUpgrade => "websocket:upgrade",
            Self::WebSocketHello => "websocket:hello",
            Self::WebSocketActive => "websocket:active",
        }
    }
}

pub fn mark(stage: Stage) {
    embassy_rp::pac::WATCHDOG
        .scratch7()
        .write(|value| *value = stage as u32);
    log::info!("health: stage={}", stage.label());
}

pub fn spawn(spawner: &Spawner, peripheral: Peri<'static, WATCHDOG>) -> bool {
    crate::log_spawn(spawner, "watchdog_task", watchdog_task(peripheral))
}

#[embassy_executor::task]
async fn watchdog_task(peripheral: Peri<'static, WATCHDOG>) -> ! {
    let mut watchdog = Watchdog::new(peripheral);
    let reset_reason = watchdog.reset_reason();
    let retained_stage = if watchdog.get_scratch(SCRATCH_MAGIC_INDEX) == SCRATCH_MAGIC {
        Stage::from_raw(watchdog.get_scratch(SCRATCH_STAGE_INDEX))
    } else {
        None
    };

    watchdog.set_scratch(SCRATCH_MAGIC_INDEX, SCRATCH_MAGIC);
    watchdog.set_scratch(SCRATCH_STAGE_INDEX, Stage::Boot as u32);
    watchdog.pause_on_debug(true);
    watchdog.start(WATCHDOG_TIMEOUT);

    // Give USB CDC time to enumerate before reporting a retained failure.
    Timer::after_secs(2).await;
    let mut ticks = 0u8;
    loop {
        if reset_reason == Some(ResetReason::TimedOut)
            && ticks < 60
            && ticks.is_multiple_of(5)
            && let Some(stage) = retained_stage
        {
            log::error!(
                "watchdog: recovered from executor stall; last_stage={}",
                stage.label()
            );
        }
        watchdog.feed(WATCHDOG_TIMEOUT);
        Timer::after(WATCHDOG_FEED_INTERVAL).await;
        // Repeat for one minute so a CDC client can reconnect after USB reset.
        ticks = ticks.saturating_add(1).min(60);
    }
}

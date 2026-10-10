//! Exclusive, explicitly armed CDC bootstrap. The control task remains the only CDC owner.
use core::{cell::RefCell, fmt::Write as _};
use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};
use embassy_time::{Duration, Instant, WithTimeout};
use embassy_usb::{class::cdc_acm::CdcAcmClass, driver::Driver};
use firmware_exec::jobs::JobHandle;
use portable_atomic::{AtomicBool, Ordering};

use crate::bootstrap_core::{self, Request, RequestDecoder, Response, Session};

include!(concat!(env!("OUT_DIR"), "/cdc_bootstrap.rs"));
static INSTALLER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cdc_installer.sh"));
pub static CAN_ARM: AtomicBool = AtomicBool::new(false);
static RESERVED: AtomicBool = AtomicBool::new(false);
static CANCEL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Armed,
    Downloading,
    Verified,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Armed => "armed",
            Self::Downloading => "downloading",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Armed => "CDC armed; enter command within 30s",
            Self::Downloading => "CDC installing; Y stops transfer",
            Self::Verified => "CDC verified; waiting for agent",
            Self::Failed => "CDC failed; retry or use Control-C",
            Self::Cancelled => "CDC stopped; use Terminal Control-C",
        }
    }
}

struct State {
    generation: u32,
    owner: Option<JobHandle>,
    armed_at: u64,
    phase: Phase,
}
static STATE: Mutex<CriticalSectionRawMutex, RefCell<State>> = Mutex::new(RefCell::new(State {
    generation: 0,
    owner: None,
    armed_at: 0,
    phase: Phase::Idle,
}));

pub fn phase() -> Phase {
    STATE.lock(|s| s.borrow().phase)
}
pub fn active() -> bool {
    RESERVED.load(Ordering::Acquire)
}
pub fn available() -> bool {
    option_env!("PICO_USB_SERIAL").is_some() && ARTIFACT.is_some()
}

/// Called synchronously before HID submission; no executor task can run between arm and bind.
pub fn arm() -> Option<u32> {
    if !available()
        || !CAN_ARM.load(Ordering::Acquire)
        || crate::capabilities::host_agent_present()
        || !super::ctrl::CTRL_CHAN.is_empty()
        || super::hid::active_job().is_some()
    {
        return None;
    }
    let generation = STATE.lock(|s| {
        let mut s = s.borrow_mut();
        if matches!(s.phase, Phase::Armed | Phase::Downloading) {
            return None;
        }
        let generation = s.generation.checked_add(1)?;
        CANCEL.reset();
        s.generation = generation;
        s.owner = None;
        s.armed_at = Instant::now().as_millis();
        s.phase = Phase::Armed;
        RESERVED.store(true, Ordering::Release);
        CAN_ARM.store(false, Ordering::Release);
        Some(generation)
    });
    if generation.is_some() {
        WAKE.signal(());
        log::info!("usb: CDC bootstrap armed");
        publish_status();
    }
    generation
}

pub fn bind(generation: u32, handle: JobHandle) {
    STATE.lock(|s| {
        let mut s = s.borrow_mut();
        if s.generation == generation {
            s.owner = Some(handle);
        }
    });
}

pub fn finish(generation: u32, phase: Phase) {
    STATE.lock(|s| {
        let mut s = s.borrow_mut();
        if s.generation == generation && matches!(s.phase, Phase::Armed | Phase::Downloading) {
            s.phase = phase;
            s.owner = None;
        }
    });
    publish_status();
}

pub fn release() {
    RESERVED.store(false, Ordering::Release);
    publish_status();
}

pub fn cancel() -> bool {
    let cancelled = STATE.lock(|s| {
        let mut s = s.borrow_mut();
        if !matches!(s.phase, Phase::Armed | Phase::Downloading) {
            return false;
        }
        s.phase = Phase::Cancelled;
        s.owner = None;
        true
    });
    if cancelled {
        CANCEL.signal(());
        publish_status();
    }
    cancelled
}

fn publish_status() {
    let _ = crate::http::transfer::queue_text(crate::capabilities::hello_json());
}

pub fn keyboard_failed(handle: JobHandle) {
    let matches = STATE.lock(|s| s.borrow().owner == Some(handle));
    if matches {
        cancel();
    }
}

pub fn armed() -> Option<(u32, u64)> {
    STATE.lock(|s| {
        let s = s.borrow();
        (s.phase == Phase::Armed).then_some((s.generation, s.armed_at))
    })
}

fn live(generation: u32, armed_at: u64) -> bool {
    STATE.lock(|s| {
        let s = s.borrow();
        s.generation == generation && matches!(s.phase, Phase::Armed | Phase::Downloading)
    }) && Instant::now().as_millis().saturating_sub(armed_at) < 120_000
}

async fn request<'d, D: Driver<'d>>(
    class: &mut CdcAcmClass<'d, D>,
    generation: u32,
    armed_at: u64,
    first: bool,
) -> Result<Request, ()> {
    let deadline = if first {
        armed_at.saturating_add(30_000)
    } else {
        Instant::now().as_millis().saturating_add(5_000)
    };
    let mut decoder = RequestDecoder::new();
    let mut packet = [0u8; 64];
    loop {
        if !live(generation, armed_at)
            || Instant::now().as_millis() >= deadline
            || (!first && !class.dtr())
        {
            return Err(());
        }
        match select(
            CANCEL.wait(),
            class
                .read_packet(&mut packet)
                .with_timeout(Duration::from_millis(100)),
        )
        .await
        {
            Either::First(_) => return Err(()),
            Either::Second(Err(_)) => continue,
            Either::Second(Ok(Err(_))) => return Err(()),
            Either::Second(Ok(Ok(n))) => {
                if let Some(request) = decoder.packet(&packet[..n]).map_err(|_| ())? {
                    if !class.dtr() {
                        return Err(());
                    }
                    return Ok(request);
                }
            }
        }
    }
}

async fn send<'d, D: Driver<'d>>(
    class: &mut CdcAcmClass<'d, D>,
    bytes: &[u8],
    generation: u32,
    armed_at: u64,
    terminate: bool,
) -> Result<(), ()> {
    for chunk in bytes.chunks(64) {
        if !live(generation, armed_at) || !class.dtr() {
            return Err(());
        }
        match select(
            CANCEL.wait(),
            class
                .write_packet(chunk)
                .with_timeout(Duration::from_secs(5)),
        )
        .await
        {
            Either::Second(Ok(Ok(()))) => (),
            _ => return Err(()),
        }
    }
    if terminate && bytes.len().is_multiple_of(64) {
        match select(
            CANCEL.wait(),
            class.write_packet(&[]).with_timeout(Duration::from_secs(5)),
        )
        .await
        {
            Either::Second(Ok(Ok(()))) => (),
            _ => return Err(()),
        }
    }
    Ok(())
}

pub async fn run<'d, D: Driver<'d>>(
    class: &mut CdcAcmClass<'d, D>,
    generation: u32,
    armed_at: u64,
) -> Result<(), ()> {
    let artifact = ARTIFACT.as_ref().ok_or(())?;
    let image = super::msc::image();
    if !artifact.valid(image) || class.max_packet_size() != 64 {
        return Err(());
    }
    let mut session = Session::new(artifact.size);
    let mut first = true;
    loop {
        let req = request(class, generation, armed_at, first).await?;
        if req == Request::Abort {
            finish(generation, Phase::Cancelled);
            return Err(());
        }
        let response = session.accept(req).ok_or(())?;
        first = false;
        match response {
            Response::Installer => send(class, INSTALLER, generation, armed_at, true).await?,
            Response::Manifest => {
                STATE.lock(|s| {
                    let mut s = s.borrow_mut();
                    if s.generation == generation {
                        s.phase = Phase::Downloading;
                    }
                });
                publish_status();
                let mut manifest: heapless::String<128> = heapless::String::new();
                writeln!(
                    manifest,
                    "C1 arm64 {} {} {}",
                    artifact.size,
                    bootstrap_core::BLOCK_BYTES,
                    artifact.digest
                )
                .map_err(|_| ())?;
                send(class, manifest.as_bytes(), generation, armed_at, true).await?;
            }
            Response::Block { offset, length } => {
                let mut sent = 0;
                while sent < length {
                    let chunk = artifact
                        .slice(image, offset + sent, (length - sent).min(64))
                        .ok_or(())?;
                    if chunk.is_empty() {
                        return Err(());
                    }
                    send(class, chunk, generation, armed_at, false).await?;
                    sent += chunk.len();
                }
                if length.is_multiple_of(64) {
                    send(class, &[], generation, armed_at, true).await?;
                }
            }
            Response::Complete => {
                send(class, b"OK1\n", generation, armed_at, true).await?;
                return Ok(());
            }
        }
    }
}

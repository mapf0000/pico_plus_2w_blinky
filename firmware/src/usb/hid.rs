use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub use bytecode_constants::MAX_BYTECODE;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::ThreadModeRawMutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use firmware_exec::jobs::{Controller, JobHandle};
use heapless::{String, Vec};

use crate::http::transfer::{self, TRANSFER_TEXT_MAX};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Local,
    Browser {
        session: u32,
        request_id: u64,
        process_id: u64,
        effect_id: u64,
    },
}

#[allow(
    clippy::large_enum_variant,
    reason = "one bounded 4096-byte browser effect; local presets remain in flash"
)]
enum Program {
    Browser(Vec<u8, MAX_BYTECODE>),
    Preset(&'static [u8]),
}
impl Program {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Browser(bytes) => bytes,
            Self::Preset(bytes) => bytes,
        }
    }
}
struct Command {
    handle: JobHandle,
    program: Program,
}

static JOBS: Mutex<ThreadModeRawMutex, RefCell<Controller<Owner>>> =
    Mutex::new(RefCell::new(Controller::new()));
static COMMANDS: Channel<ThreadModeRawMutex, Command, 1> = Channel::new();
static CANCEL_WAKE: Signal<ThreadModeRawMutex, ()> = Signal::new();
static LINK_EPOCH: AtomicU32 = AtomicU32::new(0);
static LINK_DOWN: Signal<ThreadModeRawMutex, ()> = Signal::new();
#[derive(Debug)]
struct BrowserCompletion {
    handle: JobHandle,
    session: u32,
    result: HidResult,
}
static COMPLETIONS: Channel<ThreadModeRawMutex, BrowserCompletion, 1> = Channel::new();
pub static LOCAL_RESULT: Signal<ThreadModeRawMutex, LocalResult> = Signal::new();
pub static USB_READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidResultStatus {
    Completed,
    Rejected,
    Cancelled,
    UsbUnavailable,
}
#[derive(Clone, Copy, Debug)]
pub struct HidResult {
    pub request_id: u64,
    pub process_id: u64,
    pub effect_id: u64,
    pub status: HidResultStatus,
}
#[derive(Clone, Copy, Debug)]
pub struct LocalResult {
    pub handle: JobHandle,
    pub status: HidResultStatus,
    pub completed_at_ms: u64,
}
#[derive(Clone, Copy, Debug)]
pub enum SubmitError {
    Busy,
    UsbUnavailable,
    Invalid,
}

fn submit(owner: Owner, program: Program) -> Result<JobHandle, SubmitError> {
    if !USB_READY.load(Ordering::SeqCst) {
        return Err(SubmitError::UsbUnavailable);
    }
    firmware_exec::validate_bytecode(program.bytes()).map_err(|_| SubmitError::Invalid)?;
    JOBS.lock(|cell| {
        let mut jobs = cell.borrow_mut();
        let handle = jobs.reserve(owner).ok_or(SubmitError::Busy)?;
        // Reservation and channel insertion are synchronous and cannot race a consumer.
        if COMMANDS.try_send(Command { handle, program }).is_err() {
            jobs.begin_release(handle);
            jobs.finish(handle);
            return Err(SubmitError::Busy);
        }
        Ok(handle)
    })
}
pub fn submit_browser(
    session: u32,
    id: script_protocol::EffectId,
    program: Vec<u8, MAX_BYTECODE>,
) -> Result<JobHandle, SubmitError> {
    submit(
        Owner::Browser {
            session,
            request_id: id.request_id,
            process_id: id.process_id,
            effect_id: id.effect_id,
        },
        Program::Browser(program),
    )
}
pub fn submit_preset(program: &'static [u8]) -> Result<JobHandle, SubmitError> {
    submit(Owner::Local, Program::Preset(program))
}
pub fn active_job() -> Option<firmware_exec::jobs::Job<Owner>> {
    JOBS.lock(|cell| cell.borrow().active())
}
pub fn cancel(handle: JobHandle) -> bool {
    let accepted = JOBS.lock(|cell| cell.borrow_mut().cancel(handle));
    if accepted {
        CANCEL_WAKE.signal(());
    }
    accepted
}
pub fn stop_device() -> bool {
    active_job().is_some_and(|job| cancel(job.handle))
}
fn cancel_matching(matches: impl FnOnce(Owner) -> bool) {
    let accepted = JOBS.lock(|cell| cell.borrow_mut().cancel_matching(matches));
    if accepted {
        CANCEL_WAKE.signal(());
    }
}
pub fn cancel_browser(session: u32, process_id: u64, effect_id: u64) {
    cancel_matching(
        |owner| matches!(owner, Owner::Browser { session: s, process_id: p, effect_id: e, .. } if s == session && p == process_id && e == effect_id),
    );
}
pub fn cancel_browser_session(session: u32) {
    cancel_matching(|owner| matches!(owner, Owner::Browser { session: s, .. } if s == session));
}
async fn cancelled(handle: JobHandle) {
    loop {
        if active_job().is_some_and(|job| job.handle == handle && job.cancelled) {
            return;
        }
        CANCEL_WAKE.wait().await;
    }
}
async fn disconnected() {
    loop {
        if !USB_READY.load(Ordering::SeqCst) {
            return;
        }
        LINK_DOWN.wait().await;
    }
}

/// Bus events close admission immediately, including disconnects during a delay.
pub struct LinkHandler;
impl embassy_usb::Handler for LinkHandler {
    fn enabled(&mut self, enabled: bool) {
        if !enabled {
            link_down();
        }
    }
    fn reset(&mut self) {
        link_down();
    }
    fn configured(&mut self, configured: bool) {
        if !configured {
            link_down();
        }
    }
}
fn link_down() {
    LINK_EPOCH.fetch_add(1, Ordering::SeqCst);
    USB_READY.store(false, Ordering::SeqCst);
    LINK_DOWN.signal(());
}

pub fn result_event(result: HidResult) -> String<TRANSFER_TEXT_MAX> {
    use core::fmt::Write;
    let status = match result.status {
        HidResultStatus::Completed => "completed",
        HidResultStatus::Rejected => "rejected",
        HidResultStatus::Cancelled => "cancelled",
        HidResultStatus::UsbUnavailable => "usb_unavailable",
    };
    let mut event = String::new();
    let _ = write!(
        event,
        "{{\"event_type\":\"script/effect_result\",\"version\":1,\"request_id\":\"{:016x}\",\"process_id\":\"{:016x}\",\"effect_id\":\"{:016x}\",\"status\":\"{}\"}}",
        result.request_id, result.process_id, result.effect_id, status
    );
    event
}
fn browser_result(owner: Owner, status: HidResultStatus) -> Option<(u32, HidResult)> {
    match owner {
        Owner::Browser {
            session,
            request_id,
            process_id,
            effect_id,
        } => Some((
            session,
            HidResult {
                request_id,
                process_id,
                effect_id,
                status,
            },
        )),
        Owner::Local => None,
    }
}
fn complete(handle: JobHandle, status: HidResultStatus) {
    let job = JOBS.lock(|cell| cell.borrow_mut().claim_result(handle));
    if let Some(job) = job {
        if let Some((session, result)) = browser_result(job.owner, status) {
            // One reservation remains held until the router consumes this result.
            // claim_result succeeds once, so the one-slot channel cannot be full.
            COMPLETIONS
                .try_send(BrowserCompletion {
                    handle,
                    session,
                    result,
                })
                .expect("one reserved job publishes exactly one completion");
        } else {
            LOCAL_RESULT.signal(LocalResult {
                handle,
                status,
                completed_at_ms: Instant::now().as_millis(),
            });
            JOBS.lock(|cell| {
                cell.borrow_mut().finish(handle);
            });
        }
    }
}

/// Survives USB task shutdown and backpressures browser delivery without loss.
#[embassy_executor::task]
pub async fn completion_task() -> ! {
    loop {
        let completion = COMPLETIONS.receive().await;
        let _ =
            transfer::send_text_for_generation(completion.session, result_event(completion.result))
                .await;
        JOBS.lock(|cell| {
            cell.borrow_mut().finish(completion.handle);
        });
    }
}

/// Dropping the USB task group must also terminate reserved/queued jobs.
struct UsbLifetime;
impl Drop for UsbLifetime {
    fn drop(&mut self) {
        link_down();
        while COMMANDS.try_receive().is_ok() {}
        if let Some(job) = active_job() {
            JOBS.lock(|cell| {
                cell.borrow_mut().begin_release(job.handle);
            });
            complete(job.handle, HidResultStatus::UsbUnavailable);
        }
    }
}
pub async fn run_hid<'d, D>(mut writer: UsbHidWriter<'d, D, 8>) -> !
where
    D: embassy_usb::driver::Driver<'d>,
{
    let _lifetime = UsbLifetime;
    loop {
        // Readiness is published only after a successful all-zero report.
        if !USB_READY.load(Ordering::SeqCst) {
            writer.ready().await;
            let epoch = LINK_EPOCH.load(Ordering::SeqCst);
            LINK_DOWN.reset();
            if !matches!(
                with_timeout(
                    Duration::from_millis(250),
                    firmware_exec::release_all_checked(&mut writer)
                )
                .await,
                Ok(Ok(()))
            ) {
                Timer::after_millis(100).await;
                continue;
            }
            if LINK_EPOCH.load(Ordering::SeqCst) != epoch {
                continue;
            }
            USB_READY.store(true, Ordering::SeqCst);
        }
        let command = match select(COMMANDS.receive(), disconnected()).await {
            Either::First(command) => command,
            Either::Second(()) => continue,
        };
        JOBS.lock(|cell| {
            cell.borrow_mut().start(command.handle);
        });
        let status = match select3(
            cancelled(command.handle),
            disconnected(),
            firmware_exec::exec_bytecode(&mut writer, command.program.bytes()),
        )
        .await
        {
            Either3::First(()) => HidResultStatus::Cancelled,
            Either3::Second(()) => HidResultStatus::UsbUnavailable,
            Either3::Third(Ok(())) => HidResultStatus::Completed,
            Either3::Third(Err(firmware_exec::ExecError::Usb(_))) => {
                HidResultStatus::UsbUnavailable
            }
            Either3::Third(Err(firmware_exec::ExecError::Decode(_))) => HidResultStatus::Rejected,
        };
        let was_cancelled = JOBS
            .lock(|cell| cell.borrow_mut().begin_release(command.handle))
            .unwrap_or(false);
        let mut status = if was_cancelled {
            HidResultStatus::Cancelled
        } else {
            status
        };
        let clean = matches!(
            with_timeout(
                Duration::from_millis(250),
                firmware_exec::release_all_checked(&mut writer)
            )
            .await,
            Ok(Ok(()))
        );
        if !clean || status == HidResultStatus::UsbUnavailable {
            link_down();
        }
        if !clean && status == HidResultStatus::Completed {
            status = HidResultStatus::UsbUnavailable;
        }
        complete(command.handle, status);
    }
}

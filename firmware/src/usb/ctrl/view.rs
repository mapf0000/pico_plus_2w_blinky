use super::CtrlCommand;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use portable_atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

pub static CTRL_CHAN: Channel<ThreadModeRawMutex, CtrlCommand, 8> = Channel::new();
pub static CTRL_READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferRelayMode {
    RelayToBrowser,
    SimulationDrop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferViewState {
    Idle,
    Open,
    Progress,
    Finished,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug)]
pub struct TransferViewSnapshot {
    pub mode: TransferRelayMode,
    pub state: TransferViewState,
    pub transfer_id: u64,
    pub total_size: u64,
    pub received_size: u64,
    pub chunk_count: u32,
    pub finished_chunks: u32,
}

const MODE_RELAY_TO_BROWSER: u8 = 0;
const MODE_SIMULATION_DROP: u8 = 1;

const VIEW_STATE_IDLE: u8 = 0;
const VIEW_STATE_OPEN: u8 = 1;
const VIEW_STATE_PROGRESS: u8 = 2;
const VIEW_STATE_FINISHED: u8 = 3;
const VIEW_STATE_FAILED: u8 = 4;
const VIEW_STATE_ABORTED: u8 = 5;

pub(super) static TRANSFER_MODE: AtomicU8 = AtomicU8::new(MODE_RELAY_TO_BROWSER);
pub(super) static TRANSFER_STATE: AtomicU8 = AtomicU8::new(VIEW_STATE_IDLE);
pub(super) static TRANSFER_ID: AtomicU64 = AtomicU64::new(0);
pub(super) static TRANSFER_TOTAL_SIZE: AtomicU64 = AtomicU64::new(0);
pub(super) static TRANSFER_RECEIVED_SIZE: AtomicU64 = AtomicU64::new(0);
pub(super) static TRANSFER_CHUNK_COUNT: AtomicU32 = AtomicU32::new(0);
pub(super) static TRANSFER_FINISHED_CHUNKS: AtomicU32 = AtomicU32::new(0);

pub fn transfer_relay_mode() -> TransferRelayMode {
    mode_from_raw(TRANSFER_MODE.load(Ordering::Acquire))
}

pub fn set_transfer_relay_mode(mode: TransferRelayMode) {
    TRANSFER_MODE.store(mode_to_raw(mode), Ordering::Release);
}

pub fn transfer_view_snapshot() -> TransferViewSnapshot {
    TransferViewSnapshot {
        mode: transfer_relay_mode(),
        state: view_state_from_raw(TRANSFER_STATE.load(Ordering::Acquire)),
        transfer_id: TRANSFER_ID.load(Ordering::Acquire),
        total_size: TRANSFER_TOTAL_SIZE.load(Ordering::Acquire),
        received_size: TRANSFER_RECEIVED_SIZE.load(Ordering::Acquire),
        chunk_count: TRANSFER_CHUNK_COUNT.load(Ordering::Acquire),
        finished_chunks: TRANSFER_FINISHED_CHUNKS.load(Ordering::Acquire),
    }
}

#[inline]
fn mode_from_raw(raw: u8) -> TransferRelayMode {
    match raw {
        MODE_SIMULATION_DROP => TransferRelayMode::SimulationDrop,
        _ => TransferRelayMode::RelayToBrowser,
    }
}

#[inline]
fn mode_to_raw(mode: TransferRelayMode) -> u8 {
    match mode {
        TransferRelayMode::RelayToBrowser => MODE_RELAY_TO_BROWSER,
        TransferRelayMode::SimulationDrop => MODE_SIMULATION_DROP,
    }
}

#[inline]
fn view_state_from_raw(raw: u8) -> TransferViewState {
    match raw {
        VIEW_STATE_OPEN => TransferViewState::Open,
        VIEW_STATE_PROGRESS => TransferViewState::Progress,
        VIEW_STATE_FINISHED => TransferViewState::Finished,
        VIEW_STATE_FAILED => TransferViewState::Failed,
        VIEW_STATE_ABORTED => TransferViewState::Aborted,
        _ => TransferViewState::Idle,
    }
}

#[inline]
pub(super) fn set_transfer_view_state(state: TransferViewState) {
    let raw = match state {
        TransferViewState::Idle => VIEW_STATE_IDLE,
        TransferViewState::Open => VIEW_STATE_OPEN,
        TransferViewState::Progress => VIEW_STATE_PROGRESS,
        TransferViewState::Finished => VIEW_STATE_FINISHED,
        TransferViewState::Failed => VIEW_STATE_FAILED,
        TransferViewState::Aborted => VIEW_STATE_ABORTED,
    };
    TRANSFER_STATE.store(raw, Ordering::Release);
}

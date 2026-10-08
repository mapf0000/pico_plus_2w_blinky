//! Device-service adapters. Models and rendering never access these globals.

use core::sync::atomic::Ordering;

use crate::{
    display_core::{Controller, Preset, model::*},
    http::transfer,
    usb::{
        ctrl::{self, CTRL_CHAN, CTRL_READY, CtrlCommand, TransferRelayMode, TransferViewState},
        hid::{self, HidResultStatus, Owner, SubmitError},
    },
};

include!(concat!(env!("OUT_DIR"), "/keyboard_presets.rs"));

pub fn presets() -> &'static [Preset] {
    PRESETS
}

pub fn snapshot(now_ms: u64) -> Snapshot {
    let keyboard = match hid::active_job() {
        Some(job) => KeyboardSnapshot::Busy {
            local: job.owner == Owner::Local,
            phase: job.phase,
            cancelled: job.cancelled,
        },
        None if hid::USB_READY.load(Ordering::SeqCst) => KeyboardSnapshot::Ready,
        None => KeyboardSnapshot::Unavailable,
    };
    let view = ctrl::transfer_view_snapshot();
    Snapshot {
        now_ms,
        keyboard,
        agent_present: crate::capabilities::host_agent_present(),
        transfer: TransferSnapshot {
            usb_ready: CTRL_READY.load(Ordering::Acquire),
            browser_connected: transfer::has_active_client(),
            simulation: view.mode == TransferRelayMode::SimulationDrop,
            state: match view.state {
                TransferViewState::Idle => TransferState::Idle,
                TransferViewState::Open => TransferState::Open,
                TransferViewState::Progress => TransferState::Progress,
                TransferViewState::Finished => TransferState::Finished,
                TransferViewState::Failed => TransferState::Failed,
                TransferViewState::Aborted => TransferState::Aborted,
            },
            id: view.transfer_id,
            total_bytes: view.total_size,
            received_bytes: view.received_size,
            total_chunks: view.chunk_count,
            finished_chunks: view.finished_chunks,
        },
    }
}

pub fn take_completion() -> Option<Completion> {
    hid::LOCAL_RESULT.try_take().map(|result| Completion {
        handle: result.handle,
        completed_at_ms: result.completed_at_ms,
        status: match result.status {
            HidResultStatus::Completed => CompletionStatus::Completed,
            HidResultStatus::Cancelled => CompletionStatus::Cancelled,
            HidResultStatus::Rejected => CompletionStatus::Rejected,
            HidResultStatus::UsbUnavailable => CompletionStatus::UsbUnavailable,
        },
    })
}

pub fn execute(controller: &mut Controller, intent: Intent) {
    match intent {
        Intent::RunPreset(index) => {
            if let Some(preset) = PRESETS.get(index) {
                let result = match hid::submit_preset(preset.program) {
                    Ok(handle) => Submission::Accepted(handle),
                    Err(SubmitError::Busy) => Submission::Busy,
                    Err(SubmitError::UsbUnavailable) => Submission::UsbUnavailable,
                    Err(SubmitError::Invalid) => Submission::Invalid,
                };
                controller.payloads.submitted(result, preset.launches_agent);
            }
        }
        Intent::Agent(action) => {
            let ready = controller.transfer.snapshot.usb_ready;
            let command = match action {
                AgentAction::Status => CtrlCommand::RequestStatus,
                AgentAction::Whoami => CtrlCommand::Execute { command: "whoami" },
                AgentAction::Hostname => CtrlCommand::Execute {
                    command: "hostname",
                },
                AgentAction::OpenBrowser => CtrlCommand::Execute {
                    command: "open https://google.com",
                },
                AgentAction::Credentials => CtrlCommand::RequestDbCredentials {
                    prompt: "Database credentials",
                },
            };
            let queued = ready && CTRL_CHAN.try_send(command).is_ok();
            controller.agent.submitted(action, ready, queued);
            if queued {
                log::info!("host agent action queued: {}", action.name());
            }
        }
        Intent::RequireEncryptedRelay => {
            ctrl::set_transfer_relay_mode(TransferRelayMode::RelayToBrowser)
        }
    }
}

use super::*;
use crate::{
    Capabilities, Device, MAX_DIAGNOSTICS, Status,
    mock::{LATENCY, MockTransport, Scenario},
};

fn connected(scenario: Scenario) -> (Client<MockTransport>, Duration) {
    let mut client = Client::new(MockTransport::new(scenario));
    client.dispatch(Action::Scan, Duration::ZERO);
    client.tick(LATENCY);
    client.dispatch(Action::Connect("mock-pico-1".into()), LATENCY);
    let now = LATENCY * 2;
    client.tick(now);
    (client, now)
}

fn ready() -> (Client<MockTransport>, Duration) {
    let (mut client, now) = connected(Scenario::Normal);
    client.dispatch(Action::Acquire, now);
    let now = now + LATENCY;
    client.tick(now);
    assert!(client.snapshot().can_send());
    (client, now)
}

fn text(delay_ms: u32) -> Action {
    Action::SendText {
        text: "Hi".into(),
        layout: "win_en-US".into(),
        delay_ms,
    }
}

#[test]
fn requires_control_and_usb_before_keyboard_submission() {
    let (mut client, now) = connected(Scenario::Normal);
    client.dispatch(text(0), now);
    assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
    assert!(!client.snapshot().job.active());

    let (mut client, now) = connected(Scenario::UsbUnavailable);
    client.dispatch(Action::Acquire, now);
    client.tick(now + LATENCY);
    client.dispatch(text(0), now + LATENCY);
    assert_eq!(client.snapshot().last_error, Some(Failure::UsbUnavailable));
    assert!(!client.snapshot().job.active());
}

#[test]
fn validates_real_envelope_and_bytecode_then_completes_once() {
    let (mut client, now) = ready();
    client.dispatch(text(0), now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.tick(now + LATENCY);
    assert_eq!(client.snapshot().job, Job::Running);
    client.tick(now + LATENCY * 2);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Completed));
    assert!(client.snapshot().can_send());
    // Duplicate completion cannot overwrite a later effect's state.
    client.dispatch(text(5000), now + LATENCY * 2);
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Error(Failure::Busy),
        },
        now + LATENCY * 2,
    );
    assert_eq!(client.snapshot().job, Job::Sending);
}

#[test]
fn cancellation_bypasses_pending_effect_and_prevents_later_completion() {
    let (mut client, now) = ready();
    client.dispatch(text(5000), now);
    client.tick(now + LATENCY);
    client.dispatch(Action::Cancel, now + LATENCY);
    assert_eq!(client.snapshot().job, Job::Cancelling);
    client.tick(now + LATENCY * 2);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Cancelled));
    // Poll through the original completion time; no completed event survives.
    client.tick(now + Duration::from_secs(6));
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Cancelled));
}

#[test]
fn release_and_disconnect_clear_control_and_never_resume_an_effect() {
    let (mut client, now) = ready();
    client.dispatch(Action::Release, now);
    client.tick(now + LATENCY);
    assert!(!client.snapshot().control_acquired);
    assert!(!client.snapshot().can_send());
    client.dispatch(Action::Acquire, now + LATENCY);
    client.tick(now + LATENCY * 2);
    client.dispatch(text(5000), now + LATENCY * 2);
    client.dispatch(Action::Disconnect, now + LATENCY * 2);
    client.dispatch(Action::Connect("mock-pico-1".into()), now + LATENCY * 2);
    client.tick(now + LATENCY * 3);
    assert_eq!(client.snapshot().connection, Connection::Connected);
    assert!(!client.snapshot().control_acquired);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Disconnected));
}

#[test]
fn stale_connection_events_and_wrong_effect_ids_cannot_mutate_current_job() {
    let (mut client, now) = ready();
    let old_epoch = client.epoch;
    client.dispatch(Action::Disconnect, now);
    client.dispatch(Action::Connect("mock-pico-1".into()), now);
    client.tick(now + LATENCY);
    client.receive(Event::Lost { epoch: old_epoch }, now + LATENCY);
    assert_eq!(client.snapshot().connection, Connection::Connected);
    client.dispatch(Action::Acquire, now + LATENCY);
    client.tick(now + LATENCY * 2);
    client.dispatch(text(0), now + LATENCY * 2);
    let pending = client.pending.as_ref().unwrap();
    let Kind::Effect(mut wrong) = pending.kind else {
        panic!("effect expected");
    };
    wrong.effect_id += 1;
    client.receive(
        Event::Reply {
            tag: pending.tag,
            reply: Reply::Accepted(wrong),
        },
        now + LATENCY * 2,
    );
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::InvalidData)
    );
    assert!(!client.snapshot().control_acquired);
}

#[test]
fn queued_ui_send_and_cancel_from_an_old_epoch_are_discarded() {
    let (mut client, now) = ready();
    let old_epoch = client.snapshot().epoch;
    client.dispatch(Action::Disconnect, now);
    client.dispatch(Action::Connect("mock-pico-1".into()), now);
    client.tick(now + LATENCY);
    client.dispatch(Action::Acquire, now + LATENCY);
    client.tick(now + LATENCY * 2);
    client.dispatch_for_epoch(old_epoch, text(0), now + LATENCY * 2);
    assert!(!client.snapshot().job.active());
    client.dispatch(text(5000), now + LATENCY * 2);
    client.dispatch_for_epoch(old_epoch, Action::Cancel, now + LATENCY * 2);
    assert_eq!(client.snapshot().job, Job::Sending);
    client.dispatch_for_epoch(client.snapshot().epoch, Action::Cancel, now + LATENCY * 2);
    assert_eq!(client.snapshot().job, Job::Cancelling);
}

#[test]
fn timed_out_connect_fails_closed_and_can_be_scanned_again() {
    let (mut client, now) = connected(Scenario::Timeout);
    assert_eq!(client.snapshot().connection, Connection::Connecting);
    client.tick(now + CONNECT_TIMEOUT);
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::Timeout)
    );
    assert!(!client.snapshot().pending);
    client.dispatch(Action::Scan, now + CONNECT_TIMEOUT);
    client.tick(now + CONNECT_TIMEOUT + LATENCY);
    assert_eq!(client.snapshot().devices.len(), 1);
}

#[test]
fn lost_effect_result_times_out_without_replay_or_retaining_admission() {
    let (mut client, now) = ready();
    client.dispatch(text(5000), now);
    client.transport.events_for_test_clear();
    client.dispatch(Action::Cancel, now);
    client.transport.events_for_test_clear();
    client.tick(now + CANCEL_TIMEOUT);
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::Timeout)
    );
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Disconnected));
    assert!(!client.snapshot().control_acquired);
}

#[test]
fn incompatible_and_busy_devices_do_not_grant_control() {
    let (mut client, now) = connected(Scenario::Incompatible);
    assert_eq!(client.snapshot().last_error, Some(Failure::Incompatible));
    client.dispatch(Action::Acquire, now);
    assert!(!client.snapshot().control_acquired);

    let (mut client, now) = connected(Scenario::Busy);
    client.dispatch(Action::Acquire, now);
    client.tick(now + LATENCY);
    assert_eq!(client.snapshot().last_error, Some(Failure::Busy));
    assert_eq!(client.snapshot().connection, Connection::Connected);
    assert!(!client.snapshot().control_acquired);
}

#[test]
fn scan_handles_empty_and_permission_denied() {
    for (scenario, expected) in [
        (Scenario::Empty, Connection::Disconnected),
        (
            Scenario::PermissionDenied,
            Connection::Failed(Failure::PermissionDenied),
        ),
    ] {
        let mut client = Client::new(MockTransport::new(scenario));
        client.dispatch(Action::Scan, Duration::ZERO);
        client.tick(LATENCY);
        assert_eq!(client.snapshot().connection, expected);
        assert!(client.snapshot().devices.is_empty());
    }
}

#[test]
fn link_loss_clears_admission_and_ownership() {
    let (mut client, now) = connected(Scenario::LinkLoss);
    client.dispatch(Action::Acquire, now);
    client.tick(now + LATENCY);
    client.dispatch(text(5000), now + LATENCY);
    client.tick(now + LATENCY * 3);
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::LinkLost)
    );
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Disconnected));
    assert!(!client.snapshot().control_acquired);
}

#[test]
fn rejects_unbounded_metadata_and_mismatched_reply_kinds() {
    let mut client = Client::new(MockTransport::new(Scenario::Normal));
    client.dispatch(Action::Scan, Duration::ZERO);
    let tag = client.pending.as_ref().unwrap().tag;
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Devices(vec![Device {
                id: "x".repeat(crate::MAX_METADATA_BYTES + 1),
                name: "Pico".into(),
            }]),
        },
        LATENCY,
    );
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::InvalidData)
    );

    client.dispatch(Action::Scan, LATENCY);
    let tag = client.pending.as_ref().unwrap().tag;
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Connected(Capabilities {
                upload_timeout: Duration::ZERO,
                read_only: false,
                firmware: "mock".into(),
                script_version: 1,
                layouts: vec!["win_en-US".into()],
                status: Status::default(),
                overview_supported: false,
                overview: None,
            }),
        },
        LATENCY,
    );
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::InvalidData)
    );
}

#[test]
fn diagnostics_stay_bounded_and_never_contain_typed_text() {
    let (mut client, now) = ready();
    let secret = "private text example";
    client.dispatch(
        Action::SendText {
            text: secret.into(),
            layout: "win_en-US".into(),
            delay_ms: 5000,
        },
        now,
    );
    for _ in 0..MAX_DIAGNOSTICS * 2 {
        client.dispatch(Action::Acquire, now);
    }
    assert_eq!(client.snapshot().diagnostics.len(), MAX_DIAGNOSTICS);
    assert!(client.snapshot().discarded_diagnostics > 0);
    assert!(
        client
            .snapshot()
            .diagnostics
            .iter()
            .all(|entry| !entry.message.contains(secret))
    );
    assert!(client.snapshot().job.active());
}

#[test]
fn lowering_bounds_text_and_bytecode_and_respects_advertised_layouts() {
    assert!(keyboard::text_effect("Hello from Pico!".into(), "mac_de-DE", 5000).is_ok());
    for text in [
        String::new(),
        "a".repeat(1025),
        "a".repeat(1024),
        "🙂".into(),
    ] {
        assert!(keyboard::text_effect(text, "win_en-US", 0).is_err());
    }
    assert!(keyboard::text_effect("Hi".into(), "win_en-US", 5001).is_err());
    assert!(keyboard::text_effect("Hi".into(), "unknown", 0).is_err());
    let effect = keyboard::text_effect("a".repeat(650), "win_en-US", 0).unwrap();
    assert!(effect.bytecode.len() <= script_protocol::MAX_BYTECODE);
    let id = script_protocol::EffectId {
        request_id: 1,
        process_id: 2,
        effect_id: 3,
    };
    let frame = keyboard::envelope(id, &effect.bytecode).unwrap();
    assert!(
        matches!(script_protocol::decode(&frame), Ok(script_protocol::Message::Run { id: other, .. }) if other == id)
    );
    assert!(firmware_exec::validate_bytecode(&effect.bytecode).is_ok());

    let (mut client, now) = ready();
    client.snapshot.capabilities.as_mut().unwrap().layouts = vec!["win_en-US".into()];
    client.dispatch(
        Action::SendText {
            text: "Hi".into(),
            layout: "mac_de-DE".into(),
            delay_ms: 0,
        },
        now,
    );
    assert_eq!(
        client.snapshot().last_error,
        Some(Failure::UnsupportedLayout)
    );
    assert!(!client.snapshot().job.active());
}

#[test]
fn read_only_session_rejects_control_but_polls_status_and_disconnects_on_failure() {
    let (mut client, now) = connected(Scenario::Normal);
    let caps = client.snapshot.capabilities.as_mut().unwrap();
    caps.read_only = true;
    caps.layouts.clear();
    caps.script_version = 0;
    assert!(caps.valid() && caps.compatible());
    for action in [Action::Acquire, Action::Release, text(0)] {
        client.dispatch(action, now);
        assert_eq!(client.snapshot().last_error, Some(Failure::ReadOnly));
        assert!(!client.snapshot().pending);
        assert!(!client.snapshot().can_send());
    }
    let later = now + STATUS_INTERVAL;
    client.tick(later);
    let tag = client.pending.as_ref().unwrap().tag;
    assert!(matches!(
        client.pending.as_ref().unwrap().kind,
        Kind::Status
    ));
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Error(Failure::LinkLost),
        },
        later,
    );
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::LinkLost)
    );
    assert!(client.snapshot().capabilities.is_none());
    assert!(!client.snapshot().pending);
}

#[test]
fn usb_controls_require_control_and_block_keyboard_until_ready() {
    let (mut client, now) = connected(Scenario::Normal);
    client.dispatch(Action::SetUsbEnabled(false), now);
    assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
    let (mut client, now) = ready();
    client.dispatch(Action::SetUsbEnabled(false), now);
    client.tick(now + LATENCY);
    assert!(
        !client
            .snapshot()
            .capabilities
            .as_ref()
            .unwrap()
            .status
            .usb_enabled
    );
    assert!(!client.snapshot().can_send());
    client.dispatch(Action::SetUsbEnabled(true), now + LATENCY);
    client.tick(now + LATENCY * 2);
    assert!(client.snapshot().can_send());
}

#[test]
fn background_reads_keep_controls_stable_and_preserve_actionable_errors() {
    let (mut client, mut now) = ready();
    client.dispatch(Action::Acquire, now);
    assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
    for _ in 0..5 {
        now += STATUS_INTERVAL;
        client.tick(now);
        assert!(client.snapshot().refreshing);
        assert!(!client.snapshot().pending);
        assert!(client.snapshot().can_send());
        assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
        now += LATENCY;
        client.tick(now);
        assert!(!client.snapshot().refreshing);
        assert!(client.snapshot().can_send());
        assert_eq!(client.snapshot().last_status_update, Some(now));
    }
}

#[test]
fn one_user_command_waits_for_refresh_and_runs_exactly_once() {
    let (mut client, now) = ready();
    let now = now + STATUS_INTERVAL;
    client.tick(now);
    let read_tag = client.pending.as_ref().unwrap().tag;
    client.dispatch(text(0), now);
    assert_eq!(client.snapshot().job, Job::Queued);
    assert!(client.snapshot().pending && client.snapshot().refreshing);
    client.dispatch(Action::SetUsbEnabled(false), now);
    assert_eq!(client.snapshot().last_error, Some(Failure::QueueFull));
    assert_eq!(client.pending.as_ref().unwrap().tag, read_tag);
    client.tick(now + LATENCY);
    assert_eq!(client.snapshot().job, Job::Sending);
    assert!(!client.snapshot().refreshing);
    assert!(client.queued.is_none());
    client.tick(now + LATENCY * 3);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Completed));
    assert!(
        client
            .snapshot()
            .capabilities
            .as_ref()
            .unwrap()
            .status
            .usb_enabled
    );
    client.tick(now + LATENCY * 4);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Completed));
}

#[test]
fn cancel_discards_queued_text_without_interrupting_read_or_sending_hid() {
    let (mut client, now) = ready();
    let now = now + STATUS_INTERVAL;
    client.tick(now);
    let read_tag = client.pending.as_ref().unwrap().tag;
    client.dispatch(text(5000), now);
    client.dispatch(Action::Cancel, now);
    assert!(client.queued.is_none());
    assert_eq!(client.pending.as_ref().unwrap().tag, read_tag);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Cancelled));
    assert!(!client.snapshot().pending);
    client.tick(now + LATENCY * 3);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Cancelled));
    assert!(client.snapshot().can_send());
}

#[test]
fn queued_text_is_revalidated_against_fresh_usb_status_and_is_bounded() {
    let (mut client, now) = ready();
    let now = now + STATUS_INTERVAL;
    client.tick(now);
    client.dispatch(
        Action::SendText {
            text: "a".repeat(keyboard::MAX_TEXT_BYTES + 1),
            layout: "win_en-US".into(),
            delay_ms: 0,
        },
        now,
    );
    assert_eq!(client.snapshot().last_error, Some(Failure::InvalidText));
    assert!(client.queued.is_none());
    client.dispatch(text(0), now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Status(Status::default()),
        },
        now + LATENCY,
    );
    assert_eq!(client.snapshot().last_error, Some(Failure::UsbUnavailable));
    assert!(!client.snapshot().job.active());
    assert!(!client.snapshot().pending);
}

#[test]
fn disconnect_timeout_and_stale_events_never_replay_queued_work() {
    for timeout in [false, true] {
        let (mut client, now) = ready();
        let now = now + STATUS_INTERVAL;
        client.tick(now);
        let tag = client.pending.as_ref().unwrap().tag;
        client.dispatch(text(5000), now);
        client.transport.events_for_test_clear();
        if timeout {
            client.tick(now + REQUEST_TIMEOUT);
            assert_eq!(
                client.snapshot().connection,
                Connection::Failed(Failure::Timeout)
            );
        } else {
            client.dispatch(Action::Disconnect, now);
        }
        assert!(client.queued.is_none());
        assert_eq!(client.snapshot().job, Job::Finished(Outcome::Disconnected));
        client.receive(
            Event::Reply {
                tag,
                reply: Reply::Status(Status::default()),
            },
            now,
        );
        client.dispatch(Action::Connect("mock-pico-1".into()), now);
        client.tick(now + LATENCY);
        client.dispatch_for_epoch(tag.epoch, text(0), now + LATENCY);
        assert!(!client.snapshot().control_acquired);
        assert!(!client.snapshot().job.active());
    }
}

#[test]
fn diagnostics_have_time_session_error_context_and_can_be_cleared_during_io() {
    let (mut client, now) = ready();
    client.dispatch(Action::Acquire, now);
    let entry = client.snapshot().diagnostics.back().unwrap();
    assert_eq!(entry.context, DiagnosticContext::Control);
    assert_eq!(entry.error, Some(Failure::NotReady));
    assert_eq!(entry.at, now);
    assert_eq!(entry.epoch, client.snapshot().epoch);
    let sequence = entry.sequence;
    let now = now + STATUS_INTERVAL;
    client.tick(now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.dispatch(Action::ClearDiagnostics, now);
    assert!(client.snapshot().diagnostics.is_empty());
    assert_eq!(client.snapshot().discarded_diagnostics, 0);
    assert_eq!(client.pending.as_ref().unwrap().tag, tag);
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Error(Failure::LinkLost),
        },
        now,
    );
    let entry = client.snapshot().diagnostics.back().unwrap();
    assert_eq!(entry.context, DiagnosticContext::Status);
    assert_eq!(entry.error, Some(Failure::LinkLost));
    assert!(entry.sequence > sequence);
}

fn overview_reply(script_version: u8) -> Reply {
    let wire = ble_protocol::Overview {
        request: 1,
        bootstrap: ble_protocol::BootstrapPhase::Idle,
        bootstrap_available: false,
        script_version,
        filesystem_version: 1,
        transfer_version: 2,
        max_bytecode: 4096,
        max_usb_payload: 2048,
        agent_age_ms: None,
        max_delay_ms: 5000,
        max_total_delay_ms: 300000,
        firmware: "0.1.0-full-build",
        agent_version: "",
        host_os: "",
        hostname: "",
    };
    let mut bytes = [0; ble_protocol::MAX_OVERVIEW_LEN];
    let length = wire.encode(&mut bytes).unwrap();
    Reply::Overview(crate::Overview::from_bytes(&bytes[..length]).unwrap())
}

#[test]
fn protected_overview_requires_control_refreshes_slowly_and_keeps_user_queue() {
    let (mut client, now) = connected(Scenario::Normal);
    client
        .snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .overview_supported = true;
    client.tick(now);
    assert!(client.pending.is_none());
    client.dispatch(Action::Acquire, now);
    let now = now + LATENCY;
    client.tick(now);
    assert_eq!(client.pending.as_ref().unwrap().kind, Kind::Overview);
    assert!(client.snapshot().can_send());
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.dispatch(Action::Release, now);
    client.receive(
        Event::Reply {
            tag,
            reply: overview_reply(1),
        },
        now + LATENCY,
    );
    assert_eq!(
        client.snapshot().capabilities.as_ref().unwrap().firmware,
        "0.1.0-full-build"
    );
    assert_eq!(client.snapshot().last_overview_update, Some(now + LATENCY));
    assert_eq!(client.pending.as_ref().unwrap().kind, Kind::Release);
    client.tick(now + LATENCY * 2);
    assert!(!client.snapshot().control_acquired);
    client.tick(now + OVERVIEW_INTERVAL * 2);
    assert_eq!(client.pending.as_ref().unwrap().kind, Kind::Status);
}

#[test]
fn incompatible_overview_discards_queued_effect_without_execution() {
    let (mut client, now) = ready();
    client
        .snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .overview_supported = true;
    client.tick(now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.dispatch(text(0), now);
    client.receive(
        Event::Reply {
            tag,
            reply: overview_reply(2),
        },
        now + LATENCY,
    );
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::Incompatible)
    );
    assert!(client.queued.is_none());
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Disconnected));
}

#[test]
fn late_status_completion_cannot_start_a_queued_effect_after_deadline() {
    let (mut client, now) = ready();
    let now = now + STATUS_INTERVAL;
    client.tick(now);
    client.dispatch(text(0), now);
    // Leave the transport's reply queued: receive must check the deadline
    // before releasing queued work, even when a tick arrives late.
    client.tick(now + REQUEST_TIMEOUT);
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::Timeout)
    );
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Disconnected));
    assert!(client.queued.is_none());
}

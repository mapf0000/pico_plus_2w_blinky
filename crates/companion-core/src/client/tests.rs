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
fn reconnect_rediscovers_previous_device_without_reacquiring_or_replaying_effects() {
    let (mut client, now) = ready();
    client.dispatch(text(5000), now);
    client.tick(now + LATENCY);
    let old_epoch = client.snapshot.epoch;
    let now = now + LATENCY;
    client.dispatch(Action::Disconnect, now);
    client.dispatch(Action::Reconnect, now);
    assert!(client.snapshot.epoch > old_epoch);
    assert_eq!(client.snapshot.connection, Connection::Scanning);
    client.dispatch_for_epoch(old_epoch, text(0), now);
    client.tick(now + LATENCY);
    assert_eq!(client.snapshot.connection, Connection::Connecting);
    client.tick(now + LATENCY * 2);
    assert_eq!(client.snapshot.connection, Connection::Connected);
    assert!(!client.snapshot.control_acquired);
    assert!(!client.snapshot.job.active());
    assert_eq!(client.snapshot.job, Job::Finished(Outcome::Disconnected));
    assert_eq!(client.snapshot.selected.as_ref().unwrap().id, "mock-pico-1");
}

#[test]
fn reconnect_never_falls_back_to_a_different_discovered_device() {
    let (mut client, now) = ready();
    client.dispatch(Action::Disconnect, now);
    client.dispatch(Action::Reconnect, now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Devices(vec![Device {
                id: "another-pico".into(),
                name: "Other Pico".into(),
            }]),
        },
        now + LATENCY,
    );
    assert_eq!(client.snapshot.last_error, Some(Failure::DeviceNotFound));
    assert_eq!(client.snapshot.connection, Connection::Disconnected);
    assert!(!client.snapshot.control_acquired);
    assert!(client.pending.is_none());
}

#[test]
fn cancelling_reconnect_discards_its_late_scan_result() {
    let (mut client, now) = ready();
    client.dispatch(Action::Disconnect, now);
    client.dispatch(Action::Reconnect, now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.dispatch(Action::Disconnect, now);
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Devices(vec![Device {
                id: "mock-pico-1".into(),
                name: "Pico".into(),
            }]),
        },
        now + LATENCY,
    );
    assert_eq!(client.snapshot.connection, Connection::Disconnected);
    assert!(client.pending.is_none());
}

#[test]
fn replacing_credentials_rejects_live_connections_and_invalidates_old_intents() {
    let (mut client, now) = ready();
    assert!(matches!(
        client.replace_transport(MockTransport::new(Scenario::Normal), now),
        Err(Failure::NotReady)
    ));
    assert!(client.snapshot.control_acquired);
    let old_epoch = client.snapshot.epoch;
    client.dispatch(Action::Disconnect, now);
    let _old = client
        .replace_transport(MockTransport::new(Scenario::Normal), now)
        .unwrap_or_else(|_| panic!("idle replacement failed"));
    assert!(client.snapshot.epoch > old_epoch);
    assert!(!client.snapshot.control_acquired);
    assert!(client.snapshot.selected.is_none());
    assert!(client.snapshot.devices.is_empty());
    client.dispatch_for_epoch(old_epoch, Action::Scan, now);
    assert_eq!(client.snapshot.connection, Connection::Disconnected);
    client.dispatch(Action::Scan, now);
    client.tick(now + LATENCY);
    assert!(!client.snapshot.devices.is_empty());
    assert!(!client.snapshot.control_acquired);
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
                downloads_supported: false,
                upload_timeout: Duration::ZERO,
                read_only: false,
                firmware: "mock".into(),
                script_version: 1,
                layouts: vec!["win_en-US".into()],
                status: Status::default(),
                overview_supported: false,
                overview: None,
                identity_supported: false,
                filesystem_supported: false,
                identity: None,
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

fn identity_ready() -> (Client<MockTransport>, Duration) {
    let (mut client, now) = ready();
    client.dispatch(Action::SetUsbEnabled(false), now);
    client.tick(now + LATENCY);
    assert!(client.snapshot().can_save_identity());
    (client, now + LATENCY)
}
fn identity() -> crate::UsbIdentity {
    crate::UsbIdentity::new("Test vendor", "Test product").unwrap()
}

#[test]
fn identity_requires_control_and_usb_shutdown_then_persists_across_reconnect() {
    let (mut client, now) = connected(Scenario::Normal);
    client.dispatch(Action::ReadIdentity, now);
    assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
    client.dispatch(Action::SaveIdentity(identity()), now);
    assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
    client.dispatch(Action::Acquire, now);
    client.tick(now + LATENCY);
    client.dispatch(Action::SaveIdentity(identity()), now + LATENCY);
    assert_eq!(client.snapshot().last_error, Some(Failure::UsbActive));
    assert!(!client.snapshot().pending);
    client.dispatch(Action::SetUsbEnabled(false), now + LATENCY);
    client.tick(now + LATENCY * 2);
    client.dispatch(Action::SaveIdentity(identity()), now + LATENCY * 2);
    client.tick(now + LATENCY * 3);
    assert_eq!(client.snapshot().identity_update, IdentityUpdate::Saved);
    assert_eq!(
        client.snapshot().capabilities.as_ref().unwrap().identity,
        Some(identity())
    );
    client.dispatch(Action::ReadIdentity, now + LATENCY * 3);
    client.tick(now + LATENCY * 4);
    assert_eq!(client.snapshot().identity_update, IdentityUpdate::Loaded);
    client.dispatch(Action::Disconnect, now + LATENCY * 4);
    client.dispatch(Action::Connect("mock-pico-1".into()), now + LATENCY * 4);
    client.tick(now + LATENCY * 5);
    assert_eq!(
        client.snapshot().capabilities.as_ref().unwrap().identity,
        Some(identity())
    );
    assert!(!client.snapshot().can_save_identity());
}

#[test]
fn flash_failure_returns_current_identity_and_requires_reload_before_retry() {
    let (mut client, now) = identity_ready();
    let old = client
        .snapshot()
        .capabilities
        .as_ref()
        .unwrap()
        .identity
        .clone()
        .unwrap();
    client.dispatch(Action::SaveIdentity(identity()), now);
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Identity {
                identity: old.clone(),
                error: Some(Failure::Persistence),
            },
        },
        now + LATENCY,
    );
    assert_eq!(
        client.snapshot().identity_update,
        IdentityUpdate::Failed(Failure::Persistence)
    );
    assert_eq!(
        client.snapshot().capabilities.as_ref().unwrap().identity,
        Some(old)
    );
    assert!(!client.snapshot().can_save_identity());
    assert!(client.snapshot().can_read_identity());
    client.dispatch(Action::SaveIdentity(identity()), now + LATENCY);
    assert!(client.pending.is_none());
    client.dispatch(Action::ReadIdentity, now + LATENCY);
    client.tick(now + LATENCY * 2);
    assert_eq!(client.snapshot().identity_update, IdentityUpdate::Loaded);
    assert!(client.snapshot().can_save_identity());
}

#[test]
fn queued_identity_save_is_revalidated_after_refresh_and_not_replayed_on_loss() {
    let (mut client, now) = identity_ready();
    let refresh = now + STATUS_INTERVAL;
    client.tick(refresh);
    assert!(client.snapshot().refreshing);
    assert!(!client.snapshot().pending);
    assert!(client.snapshot().can_save_identity());
    client.dispatch(Action::SaveIdentity(identity()), refresh);
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Status(Status {
                usb_enabled: true,
                usb_ready: true,
                ..Status::default()
            }),
        },
        refresh + LATENCY,
    );
    assert_eq!(client.snapshot().last_error, Some(Failure::UsbActive));
    assert!(client.pending.is_none());

    client.dispatch(Action::SetUsbEnabled(false), refresh + LATENCY);
    client.tick(refresh + LATENCY * 2);
    client.dispatch(Action::SaveIdentity(identity()), refresh + LATENCY * 2);
    let tag = client.pending.as_ref().unwrap().tag;
    client.transport.events_for_test_clear();
    client.tick(refresh + REQUEST_TIMEOUT + LATENCY * 3);
    assert_eq!(
        client.snapshot().connection,
        Connection::Failed(Failure::Timeout)
    );
    client.receive(
        Event::Reply {
            tag,
            reply: Reply::Identity {
                identity: identity(),
                error: None,
            },
        },
        refresh + REQUEST_TIMEOUT + LATENCY * 3,
    );
    assert!(client.snapshot().capabilities.is_none());
    assert_eq!(client.snapshot().identity_update, IdentityUpdate::Unknown);
    client.dispatch(
        Action::Connect("mock-pico-1".into()),
        refresh + REQUEST_TIMEOUT + LATENCY * 3,
    );
    client.tick(refresh + REQUEST_TIMEOUT + LATENCY * 4);
    assert!(client.pending.is_none());
    assert!(!client.snapshot().control_acquired);
}

#[test]
fn negotiated_identity_is_read_automatically_without_disabling_controls() {
    let (mut client, now) = ready();
    client.snapshot.capabilities.as_mut().unwrap().identity = None;
    client.snapshot.identity_update = IdentityUpdate::Unknown;
    client.tick(now);
    assert!(matches!(
        client.pending.as_ref().unwrap().kind,
        Kind::IdentityRead(true)
    ));
    assert!(client.snapshot().refreshing);
    assert!(client.snapshot().can_send());
    client.tick(now + LATENCY);
    assert_eq!(client.snapshot().identity_update, IdentityUpdate::Loaded);
    assert!(
        client
            .snapshot()
            .capabilities
            .as_ref()
            .unwrap()
            .identity
            .is_some()
    );
    client
        .snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .identity_supported = false;
    client.dispatch(Action::ReadIdentity, now + LATENCY);
    assert_eq!(client.snapshot().last_error, Some(Failure::NotReady));
}

fn browse(path: &str) -> Action {
    Action::Browse {
        path: path.into(),
        cursor: 0,
        hidden: false,
    }
}
#[test]
fn filesystem_browsing_and_superseded_cancellation_are_scoped() {
    let (mut c, mut now) = ready();
    c.dispatch(browse(""), now);
    now += LATENCY;
    c.tick(now);
    assert_eq!(c.snapshot.filesystem.directory, "/mock");
    assert_eq!(c.snapshot.filesystem.entries.len(), 2);
    c.dispatch(browse("/first"), now);
    let old = c.pending.as_ref().unwrap().tag;
    c.dispatch(browse("/second"), now);
    now += LATENCY;
    c.tick(now);
    now += LATENCY;
    c.tick(now);
    assert_eq!(c.snapshot.filesystem.directory, "/second");
    assert!(!c.snapshot.filesystem.loading);
    c.receive(
        Event::Reply {
            tag: old,
            reply: Reply::Directory(crate::mock::mock_directory(old.request, "/old", 0, false)),
        },
        now,
    );
    assert_eq!(c.snapshot.filesystem.directory, "/second");
    c.dispatch(browse("/third"), now);
    c.dispatch(Action::CancelDirectory, now);
    now += LATENCY;
    c.tick(now);
    assert_eq!(c.snapshot.filesystem.directory, "/second");
    assert!(!c.snapshot.filesystem.loading);
    c.dispatch(Action::Disconnect, now);
    assert!(c.snapshot.filesystem.entries.is_empty());
    assert!(c.snapshot.filesystem.selected.is_none());
}
#[test]
fn filesystem_requires_capability_agent_control_and_times_out_without_replay() {
    let (mut c, mut now) = connected(Scenario::Normal);
    c.dispatch(browse(""), now);
    assert!(!c.snapshot.filesystem.loading);
    c.dispatch(Action::Acquire, now);
    now += LATENCY;
    c.tick(now);
    c.snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .filesystem_supported = false;
    c.dispatch(browse(""), now);
    assert!(!c.snapshot.filesystem.loading);
    c.snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .filesystem_supported = true;
    c.snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .status
        .host_agent_present = false;
    c.dispatch(browse(""), now);
    assert!(!c.snapshot.filesystem.loading);
    c.snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .status
        .host_agent_present = true;
    c.dispatch(browse(""), now);
    c.transport.events_for_test_clear();
    c.tick(now + Duration::from_secs(35));
    assert_eq!(c.snapshot.connection, Connection::Failed(Failure::Timeout));
    assert!(!c.snapshot.filesystem.loading);
}
#[test]
fn button_events_are_bounded_and_discarded_after_release_or_reconnect() {
    let (mut c, now) = ready();
    for i in 0..40 {
        c.receive(
            Event::Button {
                epoch: c.epoch,
                button: i % 4,
            },
            now,
        );
    }
    assert_eq!(c.snapshot.buttons.len(), 32);
    assert_eq!(c.snapshot.dropped_buttons, 8);
    c.receive(
        Event::Button {
            epoch: c.epoch + 1,
            button: 0,
        },
        now,
    );
    assert_eq!(c.snapshot.dropped_buttons, 8);
    c.dispatch(Action::Release, now);
    c.tick(now + LATENCY);
    assert!(c.snapshot.buttons.is_empty());
    c.receive(
        Event::Button {
            epoch: c.epoch,
            button: 1,
        },
        now + LATENCY,
    );
    assert!(c.snapshot.buttons.is_empty());
}

#[test]
fn general_script_bytecode_is_validated_scoped_and_cancelled_during_refresh() {
    let (mut client, now) = ready();
    client.dispatch(
        Action::SendEffect {
            bytecode: vec![0; 4097],
            owner: 7,
        },
        now,
    );
    assert_eq!(client.snapshot().last_error, Some(Failure::InvalidData));
    assert!(!client.snapshot().job.active());
    let bytecode = keyboard::text_effect("abc".into(), "win_en-US", 1000)
        .unwrap()
        .bytecode;
    client.tick(now + STATUS_INTERVAL);
    assert!(client.snapshot().refreshing);
    client.dispatch(
        Action::SendEffect { bytecode, owner: 7 },
        now + STATUS_INTERVAL,
    );
    assert_eq!(client.snapshot().job, Job::Queued);
    assert_eq!(client.snapshot().job_owner, Some(7));
    client.dispatch(Action::Cancel, now + STATUS_INTERVAL);
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Cancelled));
    client.tick(now + STATUS_INTERVAL + LATENCY);
    assert!(!client.snapshot().job.active());
}

#[test]
fn general_script_bytecode_completes_and_cannot_run_after_disconnect() {
    let (mut client, now) = ready();
    let bytecode = keyboard::text_effect("abc".into(), "win_en-US", 0)
        .unwrap()
        .bytecode;
    client.dispatch(
        Action::SendEffect {
            bytecode: bytecode.clone(),
            owner: 9,
        },
        now,
    );
    assert_eq!(client.snapshot().job_owner, Some(9));
    client.tick(now + LATENCY + Duration::from_secs(1));
    assert_eq!(client.snapshot().job, Job::Finished(Outcome::Completed));
    let old = client.snapshot().epoch;
    client.dispatch(Action::Disconnect, now + LATENCY + Duration::from_secs(1));
    client.dispatch_for_epoch(
        old,
        Action::SendEffect { bytecode, owner: 9 },
        now + Duration::from_secs(2),
    );
    assert_eq!(client.snapshot().connection, Connection::Disconnected);
    assert!(!client.snapshot().job.active());
}

#[test]
fn download_cancel_discards_queued_work_and_never_retires_a_keyboard_job() {
    use crate::download::Command;
    let (mut c, now) = ready();
    c.snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .downloads_supported = true;
    c.snapshot
        .capabilities
        .as_mut()
        .unwrap()
        .status
        .host_agent_present = true;
    c.tick(now + STATUS_INTERVAL);
    assert!(c.pending.as_ref().unwrap().kind.background());
    c.dispatch(
        Action::Download(Command::Queue {
            path: "/source".into(),
            destination: std::env::temp_dir().join("new.bin"),
        }),
        now + STATUS_INTERVAL,
    );
    assert!(c.queued.is_some());
    c.dispatch(Action::Download(Command::Cancel), now + STATUS_INTERVAL);
    assert!(c.queued.is_none());
    assert_eq!(c.pending.as_ref().unwrap().kind, Kind::Download);
    let tag = c.pending.as_ref().unwrap().tag;
    c.receive(
        Event::Reply {
            tag,
            reply: Reply::DownloadAccepted,
        },
        now + STATUS_INTERVAL,
    );
    c.dispatch(text(5000), now + STATUS_INTERVAL);
    let original = c.pending.as_ref().unwrap().tag;
    c.dispatch(Action::Download(Command::Cancel), now + STATUS_INTERVAL);
    assert_eq!(c.snapshot.last_error, Some(Failure::Busy));
    assert_eq!(c.pending.as_ref().unwrap().tag, original);
}
#[test]
fn old_download_snapshots_cannot_revive_released_or_replaced_connections() {
    use crate::download::{Session, View};
    let (mut c, now) = ready();
    let epoch = c.snapshot.epoch;
    let view = View {
        session: Session::Ready,
        ..Default::default()
    };
    c.receive(
        Event::Downloads {
            epoch,
            view: view.clone(),
        },
        now,
    );
    assert_eq!(c.snapshot.downloads.session, Session::Ready);
    c.dispatch(Action::Disconnect, now);
    c.receive(Event::Downloads { epoch, view }, now);
    assert_eq!(c.snapshot.downloads.session, Session::Idle);
}

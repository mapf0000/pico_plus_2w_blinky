use companion_core::{
    Action, Client, Connection, IdentityUpdate, Job, Outcome, UsbIdentity,
    mock::{LATENCY, MockTransport, Scenario},
};
use std::time::Duration;

/// Exercise the mock lifecycle without a window, radio, USB or wall-clock sleeps.
pub fn run() -> Result<(), &'static str> {
    let mut client = Client::new(MockTransport::new(Scenario::Normal));
    let mut now = Duration::ZERO;
    client.dispatch(Action::Scan, now);
    now += LATENCY;
    client.tick(now);
    let id = client
        .snapshot()
        .devices
        .first()
        .ok_or("mock scan failed")?
        .id
        .clone();
    client.dispatch(Action::Connect(id.clone()), now);
    now += LATENCY;
    client.tick(now);
    if client.snapshot().connection != Connection::Connected {
        return Err("mock connect failed");
    }
    client.dispatch(Action::Acquire, now);
    now += LATENCY;
    client.tick(now);
    if !client.snapshot().can_send() {
        return Err("mock control acquisition failed");
    }
    client.dispatch(
        Action::SendText {
            text: "Hi".into(),
            layout: "win_en-US".into(),
            delay_ms: 0,
        },
        now,
    );
    now += LATENCY * 2;
    client.tick(now);
    if client.snapshot().job != Job::Finished(Outcome::Completed) {
        return Err("mock completion failed");
    }
    client.dispatch(
        Action::SendText {
            text: "Hi".into(),
            layout: "win_en-US".into(),
            delay_ms: 5000,
        },
        now,
    );
    now += LATENCY;
    client.tick(now);
    client.dispatch(Action::Cancel, now);
    now += LATENCY;
    client.tick(now);
    if client.snapshot().job != Job::Finished(Outcome::Cancelled) {
        return Err("mock cancellation failed");
    }
    let original = client
        .snapshot()
        .capabilities
        .as_ref()
        .and_then(|c| c.identity.clone())
        .ok_or("mock identity unavailable")?;
    client.dispatch(Action::SetUsbEnabled(false), now);
    now += LATENCY;
    client.tick(now);
    let edited = UsbIdentity::new("Mock vendor", "Mock product").map_err(|e| e.message())?;
    client.dispatch(Action::SaveIdentity(edited.clone()), now);
    now += LATENCY;
    client.tick(now);
    if client.snapshot().identity_update != IdentityUpdate::Saved {
        return Err("mock identity save failed");
    }
    client.dispatch(Action::ReadIdentity, now);
    now += LATENCY;
    client.tick(now);
    if client
        .snapshot()
        .capabilities
        .as_ref()
        .and_then(|c| c.identity.as_ref())
        != Some(&edited)
    {
        return Err("mock identity readback failed");
    }
    client.dispatch(Action::SaveIdentity(original), now);
    now += LATENCY;
    client.tick(now);
    client.dispatch(Action::SetUsbEnabled(true), now);
    now += LATENCY;
    client.tick(now);
    client.dispatch(
        Action::Browse {
            path: String::new(),
            cursor: 0,
            hidden: false,
        },
        now,
    );
    now += LATENCY;
    client.tick(now);
    if client.snapshot().filesystem.entries.len() != 2 {
        return Err("mock filesystem listing failed");
    }
    client.dispatch(
        Action::Browse {
            path: "/".into(),
            cursor: 0,
            hidden: true,
        },
        now,
    );
    client.dispatch(Action::CancelDirectory, now);
    now += LATENCY;
    client.tick(now);
    if client.snapshot().filesystem.loading || client.snapshot().filesystem.directory != "/mock" {
        return Err("mock filesystem cancellation failed");
    }
    client.dispatch(Action::Release, now);
    now += LATENCY;
    client.tick(now);
    if client.snapshot().control_acquired {
        return Err("mock release failed");
    }
    client.dispatch(Action::Disconnect, now);
    client.dispatch(Action::Connect(id), now);
    now += LATENCY;
    client.tick(now);
    if client.snapshot().connection != Connection::Connected
        || client.snapshot().control_acquired
        || client.snapshot().job.active()
    {
        return Err("reconnect resumed control or pending effects");
    }
    Ok(())
}

use companion_core::{
    Action, Client, Connection, Job, Outcome,
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

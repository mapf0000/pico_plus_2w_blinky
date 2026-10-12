//! Actual child-process acceptance over mock device effects; never opens BLE/USB.
use crate::python::{State, Supervisor};
use companion_core::{
    Action, Client, Connection, Event, Request, Transport,
    mock::{MockTransport, Scenario},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
struct Harness {
    mock: MockTransport,
    injected: Arc<Mutex<VecDeque<Event>>>,
}
impl Transport for Harness {
    fn submit(&mut self, r: Request, now: Duration) -> Result<(), companion_core::Failure> {
        self.mock.submit(r, now)
    }
    fn poll(&mut self, now: Duration) -> Option<Event> {
        self.injected
            .lock()
            .unwrap()
            .pop_front()
            .or_else(|| self.mock.poll(now))
    }
    fn close(&mut self) {
        self.mock.close();
    }
}
async fn until<T: Transport>(
    python: &mut Supervisor,
    client: &mut Client<T>,
    clock: Instant,
    expected: impl Fn(&Supervisor, &Client<T>) -> bool,
) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(35);
    loop {
        client.tick(clock.elapsed());
        python.tick(client, clock.elapsed());
        if expected(python, client) {
            return Ok(());
        }
        if python.snapshot.state == State::Faulted {
            return Err(python.snapshot.error.clone().unwrap_or_default());
        }
        if Instant::now() > end {
            return Err(format!(
                "Python acceptance timed out (script {:?}, connection {:?}, pending {}, job {:?}, device error {:?})",
                python.snapshot.state,
                client.snapshot().connection,
                client.snapshot().pending,
                client.snapshot().job,
                client.snapshot().last_error
            ));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn connect<T: Transport>(
    python: &mut Supervisor,
    client: &mut Client<T>,
    clock: Instant,
) -> Result<(), String> {
    client.dispatch(Action::Scan, clock.elapsed());
    println!("Python check: scan");
    until(python, client, clock, |_, c| {
        c.snapshot().connection == Connection::Disconnected && !c.snapshot().pending
    })
    .await?;
    let id = client
        .snapshot()
        .devices
        .first()
        .ok_or("No Pico BLE devices found; boot firmware and scan again")?
        .id
        .clone();
    client.dispatch(Action::Connect(id), clock.elapsed());
    println!("Python check: connect");
    until(python, client, clock, |_, c| {
        c.snapshot().connection == Connection::Connected && !c.snapshot().pending
    })
    .await?;
    client.dispatch(Action::Acquire, clock.elapsed());
    println!("Python check: acquire");
    until(python, client, clock, |_, c| {
        c.snapshot().control_acquired && !c.snapshot().pending
    })
    .await
}
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let clock = Instant::now();
            let injected = Arc::new(Mutex::new(VecDeque::new()));
            let mut client = Client::new(Harness {
                mock: MockTransport::new(Scenario::Normal),
                injected: injected.clone(),
            });
            let mut python = Supervisor::new();
            let result = check(&mut python, &mut client, &injected, clock).await;
            python.shutdown(&mut client, clock.elapsed()).await;
            result.map_err(|e| e.into())
        })
}

/// Uses empty keyboard effects to exercise the real generator/KBD1/BLE executor
/// path without typing characters. Firmware still performs key-release cleanup.
pub fn run_ble(
    profile: ble_session::provisioning::Profile,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let clock = Instant::now();
        let mut client = Client::new(crate::ble::BleTransport::new(Some(profile)));
        let mut python = Supervisor::new();
        let result = async {
            connect(&mut python, &mut client, clock).await?;
            python.start("layout(\"win_en-US\")\ndef main():\n    yield require_usb()\n    for index in range(3):\n        yield text(\"\", 0)\n    yield sleep(10)\n".into(), &mut client, clock.elapsed());
            until(&mut python,&mut client,clock,|p,_|p.snapshot.state==State::Completed).await?;
            if python.snapshot.completed_effects != 3 { return Err("BLE Python effects were duplicated or lost".to_owned()); }
            client.dispatch(Action::Release,clock.elapsed());
            until(&mut python,&mut client,clock,|_,c|!c.snapshot().control_acquired&&!c.snapshot().pending).await?;
            println!("Python BLE acceptance passed: authenticated control, generator/USB check, three empty KBD1 effects and correlated completion. No characters typed, identity writes or USB toggles.");
            Ok::<(),String>(())
        }.await;
        python.shutdown(&mut client,clock.elapsed()).await;
        client.dispatch(Action::Disconnect,clock.elapsed());
        client.into_transport().shutdown().await;
        result.map_err(|e|e.into())
    })
}
async fn check(
    p: &mut Supervisor,
    c: &mut Client<Harness>,
    injected: &Arc<Mutex<VecDeque<Event>>>,
    clock: Instant,
) -> Result<(), String> {
    connect(p, c, clock).await?;
    let started = Instant::now();
    p.start("layout(\"win_en-US\")\ndef main():\n    yield require_usb()\n    yield require_host_agent()\n    yield tap(\"ENTER\")\n    yield modtap(\"CTRL+C\")\n    for i in range(3):\n        yield text(\"abc\", 0)\n    event = yield wait_event(\"button\", timeout_ms=10)\n    assert event[\"kind\"] == \"timer\" and event[\"timeout\"]\n    yield sleep(1)\n".into(),c,clock.elapsed());
    until(p, c, clock, |p, _| p.snapshot.state == State::Completed).await?;
    if p.snapshot.completed_effects != 5 {
        return Err("Python effects were duplicated or lost".into());
    }
    println!(
        "Python generator/effects passed in {:.3}s; peak Rust heap {:.2} MiB",
        started.elapsed().as_secs_f64(),
        p.snapshot.heap_peak as f64 / 1048576.0
    );

    p.start("layout(\"win_en-US\")\ndef main():\n    event = yield wait_event(\"button\", timeout_ms=3000)\n    assert event[\"button\"] == \"A\" and event[\"edge\"] == \"released\"\n    yield sleep(1)\n".into(),c,clock.elapsed());
    until(p, c, clock, |p, _| p.snapshot.state == State::WaitingEvent).await?;
    injected.lock().unwrap().push_back(Event::Button {
        epoch: c.snapshot().epoch,
        button: 0,
    });
    until(p, c, clock, |p, _| p.snapshot.state == State::Completed).await?;
    println!("Python button wait passed through the client and child IPC.");

    p.start("layout(\"win_en-US\")\ndef main():\n    remembered = 42\n    try:\n        yield text(\"a\" * 100, 50)\n    except DeviceDisconnected:\n        event = yield wait_event(\"connection\", timeout_ms=3000)\n        assert event[\"connected\"] and remembered == 42\n        yield sleep(1)\n".into(),c,clock.elapsed());
    until(p, c, clock, |p, c| {
        p.snapshot.state == State::WaitingDevice && c.snapshot().job == companion_core::Job::Running
    })
    .await?;
    c.dispatch(Action::Disconnect, clock.elapsed());
    until(p, c, clock, |p, _| p.snapshot.state == State::WaitingEvent).await?;
    connect(p, c, clock).await?;
    until(p, c, clock, |p, _| p.snapshot.state == State::Completed).await?;
    println!("Python disconnect/throw/reconnect retained state without replaying keyboard output.");

    for source in [
        "layout(\"win_en-US\")\ndef main():\n    yield sleep(1)\n    while True:\n        pass\n",
        "layout(\"win_en-US\")\ndef main():\n    huge = bytearray(256 * 1024 * 1024)\n    yield sleep(1)\n",
        "layout(\"win_en-US\")\nimport os\ndef main():\n    yield sleep(1)\n",
    ] {
        p.start(source.into(), c, clock.elapsed());
        until(p, c, clock, |p, _| p.snapshot.state == State::Faulted).await?;
        println!(
            "Python containment check: {}",
            p.snapshot.error.as_deref().unwrap_or("missing fault")
        );
    }

    p.start("layout(\"win_en-US\")\ndef main():\n    chunks = []\n    try:\n        while True:\n            chunks.append(bytearray(1024 * 1024))\n    except MemoryError:\n        chunks = []\n        yield sleep(1)\n".into(), c, clock.elapsed());
    until(p, c, clock, |p, _| p.snapshot.state == State::Completed).await?;
    if !(100 * 1024 * 1024..=python_runtime::ipc::HEAP_LIMIT).contains(&p.snapshot.heap_peak) {
        return Err("Heap budget did not bound repeated allocations".into());
    }
    println!(
        "Repeated allocation pressure recovered at {:.2} MiB peak Rust heap",
        p.snapshot.heap_peak as f64 / 1048576.0
    );

    p.start("while True:\n    pass\n".into(), c, clock.elapsed());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stopped = Instant::now();
    p.stop(c, clock.elapsed());
    p.start(
        "layout(\"win_en-US\")\ndef main():\n    yield sleep(1)\n".into(),
        c,
        clock.elapsed(),
    );
    until(p, c, clock, |p, _| p.snapshot.state == State::Completed).await?;
    println!(
        "Stop during non-yielding startup and fresh child passed in {:.3}s",
        stopped.elapsed().as_secs_f64()
    );

    p.start(
        "layout(\"win_en-US\")\ndef main():\n    yield text(\"a\" * 100, 50)\n".into(),
        c,
        clock.elapsed(),
    );
    until(p, c, clock, |p, c| {
        p.snapshot.state == State::WaitingDevice && c.snapshot().job == companion_core::Job::Running
    })
    .await?;
    p.stop(c, clock.elapsed());
    until(p, c, clock, |p, c| {
        p.snapshot.state == State::Stopped
            && c.snapshot().job == companion_core::Job::Finished(companion_core::Outcome::Cancelled)
    })
    .await?;
    println!("Python Stop cancelled only its own mock keyboard effect.");
    Ok(())
}

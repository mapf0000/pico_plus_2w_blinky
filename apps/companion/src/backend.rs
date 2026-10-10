use companion_core::{
    Action, Client, Snapshot,
    mock::{MockTransport, Scenario},
};
use eframe::egui;
use std::{
    io, thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

const COMMAND_CAPACITY: usize = 16;
const URGENT_CAPACITY: usize = 4;

struct Intent {
    epoch: u64,
    action: Action,
}

pub struct Backend {
    commands: mpsc::Sender<Intent>,
    urgent: mpsc::Sender<Intent>,
    pub snapshots: watch::Receiver<Snapshot>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Backend {
    pub fn start(scenario: Scenario, repaint: egui::Context) -> io::Result<Self> {
        let (commands, mut command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (urgent, mut urgent_rx) = mpsc::channel(URGENT_CAPACITY);
        let (snapshots_tx, snapshots) = watch::channel(Snapshot::default());
        let (shutdown, mut shutdown_rx) = oneshot::channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let thread = thread::Builder::new()
            .name("companion-backend".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let start = Instant::now();
                    let mut client = Client::new(MockTransport::new(scenario));
                    let mut ticker = tokio::time::interval(Duration::from_millis(50));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    client.dispatch(Action::Scan, Duration::ZERO);
                    let mut revision = u64::MAX;
                    loop {
                        if client.snapshot().revision != revision {
                            revision = client.snapshot().revision;
                            snapshots_tx.send_replace(client.snapshot().clone());
                            repaint.request_repaint();
                        }
                        tokio::select! {
                            biased;
                            _ = &mut shutdown_rx => break,
                            action = urgent_rx.recv() => {
                                let Some(action) = action else { break; };
                                let Intent { epoch, action } = action;
                                client.dispatch_for_epoch(epoch, action, start.elapsed());
                            }
                            _ = ticker.tick() => client.tick(start.elapsed()),
                            action = command_rx.recv() => {
                                let Some(action) = action else { break; };
                                let Intent { epoch, action } = action;
                                client.dispatch_for_epoch(epoch, action, start.elapsed());
                            }
                        }
                    }
                    client.dispatch(Action::Disconnect, start.elapsed());
                });
            })?;
        Ok(Self {
            commands,
            urgent,
            snapshots,
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    pub fn send(&self, epoch: u64, action: Action) -> Result<(), &'static str> {
        let sender = if matches!(action, Action::Cancel | Action::Disconnect) {
            &self.urgent
        } else {
            &self.commands
        };
        sender
            .try_send(Intent { epoch, action })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => "Command queue is full; try again",
                mpsc::error::TrySendError::Closed(_) => {
                    "Companion backend stopped; restart the app"
                }
            })
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // The backend contains only non-blocking operations and selects shutdown
        // before all traffic. Join releases the runtime and simulated session.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_interrupts_a_pending_connection_and_queued_commands() {
        let backend = Backend::start(Scenario::Timeout, egui::Context::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while backend.snapshots.borrow().devices.is_empty() {
            assert!(Instant::now() < deadline, "mock scan did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        let epoch = backend.snapshots.borrow().epoch;
        backend
            .send(epoch, Action::Connect("mock-pico-1".into()))
            .unwrap();
        while backend.snapshots.borrow().connection != companion_core::Connection::Connecting {
            assert!(Instant::now() < deadline, "mock connection did not start");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(backend.snapshots.borrow().pending);
        let epoch = backend.snapshots.borrow().epoch;
        for _ in 0..COMMAND_CAPACITY * 2 {
            let _ = backend.send(epoch, Action::Acquire);
        }
        let start = Instant::now();
        drop(backend);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

//! CoreBluetooth/BlueZ/WinRT worker. Only discovery, connect and GATT reads exist.
use btleplug::{
    api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter},
    platform::{Adapter, Manager, Peripheral},
};
use companion_core::{
    Capabilities, Device, Event, Failure, MAX_DEVICES, Operation, Reply, Request, Status, Transport,
};
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use uuid::Uuid;

const IO_TIMEOUT: Duration = Duration::from_secs(4);
const CLEANUP_TIMEOUT: Duration = Duration::from_millis(750);
const SCAN_TIME: Duration = Duration::from_secs(2);
const SERVICE: Uuid = Uuid::from_u128(ble_protocol::SERVICE_UUID);
const INFO: Uuid = Uuid::from_u128(ble_protocol::INFO_UUID);
const STATUS: Uuid = Uuid::from_u128(ble_protocol::STATUS_UUID);

struct Command {
    generation: u64,
    request: Request,
}

pub struct BleTransport {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    reset: watch::Sender<u64>,
    stop: Option<oneshot::Sender<()>>,
    worker: JoinHandle<()>,
}

impl BleTransport {
    pub fn new() -> Self {
        Self::spawn(Radio::default())
    }

    fn spawn(radio: impl RadioIo + Send + 'static) -> Self {
        let (commands, rx) = mpsc::channel(4);
        let (tx, events) = mpsc::channel(16);
        let (reset, reset_rx) = watch::channel(0);
        let (stop, stop_rx) = oneshot::channel();
        let worker = tokio::spawn(run(radio, rx, tx, reset_rx, stop_rx));
        Self {
            commands,
            events,
            reset,
            stop: Some(stop),
            worker,
        }
    }

    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.worker).await;
    }
}

impl Transport for BleTransport {
    fn submit(&mut self, request: Request, _: Duration) -> Result<(), Failure> {
        self.commands
            .try_send(Command {
                generation: *self.reset.borrow(),
                request,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Failure::QueueFull,
                mpsc::error::TrySendError::Closed(_) => Failure::BluetoothUnavailable,
            })
    }
    fn poll(&mut self, _: Duration) -> Option<Event> {
        self.events.try_recv().ok()
    }
    fn close(&mut self) {
        // Watch coalesces urgent resets and interrupts I/O without waiting for queue space.
        self.reset
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        while self.events.try_recv().is_ok() {}
    }
}

#[derive(Default)]
struct Radio {
    adapter: Option<Adapter>,
    devices: Vec<Device>,
    connected: Option<Peripheral>,
    scanning: bool,
}

trait RadioIo {
    fn cleanup(&mut self) -> impl std::future::Future<Output = ()> + Send;
    fn request(
        &mut self,
        operation: Operation,
    ) -> impl std::future::Future<Output = Result<Reply, Failure>> + Send;
}

impl RadioIo for Radio {
    async fn cleanup(&mut self) {
        if let Some(peripheral) = self.connected.take() {
            let _ = tokio::time::timeout(CLEANUP_TIMEOUT, peripheral.disconnect()).await;
        }
        if self.scanning {
            self.scanning = false;
            if let Some(adapter) = &self.adapter {
                let _ = tokio::time::timeout(CLEANUP_TIMEOUT, adapter.stop_scan()).await;
            }
        }
    }

    async fn request(&mut self, operation: Operation) -> Result<Reply, Failure> {
        match operation {
            Operation::Scan => {
                self.devices.clear();
                if self.adapter.is_none() {
                    let manager = Manager::new().await.map_err(failure)?;
                    self.adapter = manager
                        .adapters()
                        .await
                        .map_err(failure)?
                        .into_iter()
                        .next();
                }
                let adapter = self.adapter.as_ref().ok_or(Failure::AdapterUnavailable)?;
                self.scanning = true;
                adapter
                    .start_scan(ScanFilter {
                        services: vec![SERVICE],
                    })
                    .await
                    .map_err(failure)?;
                tokio::time::sleep(SCAN_TIME).await;
                adapter.stop_scan().await.map_err(failure)?;
                self.scanning = false;
                // Bound app-owned discovery state. Platform BLE caches are owned by btleplug.
                for peripheral in adapter
                    .peripherals()
                    .await
                    .map_err(failure)?
                    .into_iter()
                    .take(256)
                {
                    let Some(props) = peripheral.properties().await.map_err(failure)? else {
                        continue;
                    };
                    if !props.services.contains(&SERVICE) {
                        continue;
                    }
                    let device = Device {
                        id: peripheral.id().to_string(),
                        name: props.local_name.unwrap_or_else(|| "Pico BLE".into()),
                    };
                    if device.valid() && !self.devices.iter().any(|other| other.id == device.id) {
                        self.devices.push(device);
                        if self.devices.len() == MAX_DEVICES {
                            break;
                        }
                    }
                }
                Ok(Reply::Devices(self.devices.clone()))
            }
            Operation::Connect(device) => {
                if !self.devices.contains(&device) {
                    return Err(Failure::InvalidData);
                }
                let adapter = self.adapter.as_ref().ok_or(Failure::AdapterUnavailable)?;
                // CoreBluetooth removes its peripheral objects after disconnect.
                // Rediscover on every connection; never reuse an old OS handle.
                self.scanning = true;
                adapter
                    .start_scan(ScanFilter {
                        services: vec![SERVICE],
                    })
                    .await
                    .map_err(failure)?;
                tokio::time::sleep(SCAN_TIME).await;
                adapter.stop_scan().await.map_err(failure)?;
                self.scanning = false;
                let peripheral = adapter
                    .peripherals()
                    .await
                    .map_err(failure)?
                    .into_iter()
                    .find(|peripheral| peripheral.id().to_string() == device.id)
                    .ok_or(Failure::LinkLost)?;
                // Store before awaiting so cancellation also cleans up a partial connection.
                self.connected = Some(peripheral.clone());
                peripheral.connect().await.map_err(failure)?;
                peripheral.discover_services().await.map_err(failure)?;
                let info = characteristic(&peripheral, INFO)?;
                let bytes = peripheral.read(&info).await.map_err(failure)?;
                let info = ble_protocol::Info::decode(&bytes).map_err(protocol_failure)?;
                let status = read_status(&peripheral).await?;
                Ok(Reply::Connected(Capabilities {
                    read_only: true,
                    script_version: 0,
                    firmware: info.build_label().into(),
                    layouts: vec![],
                    status,
                }))
            }
            Operation::Status => {
                let peripheral = self.connected.as_ref().ok_or(Failure::LinkLost)?;
                if !peripheral.is_connected().await.map_err(failure)? {
                    return Err(Failure::LinkLost);
                }
                Ok(Reply::Status(read_status(peripheral).await?))
            }
            // No characteristic writes, subscriptions, control leases or keyboard effects.
            _ => Err(Failure::ReadOnly),
        }
    }
}

fn characteristic(
    peripheral: &Peripheral,
    uuid: Uuid,
) -> Result<btleplug::api::Characteristic, Failure> {
    peripheral
        .characteristics()
        .into_iter()
        .find(|value| {
            value.service_uuid == SERVICE
                && value.uuid == uuid
                && value.properties.contains(CharPropFlags::READ)
        })
        .ok_or(Failure::Incompatible)
}

async fn read_status(peripheral: &Peripheral) -> Result<Status, Failure> {
    let value = peripheral
        .read(&characteristic(peripheral, STATUS)?)
        .await
        .map_err(failure)?;
    let status = ble_protocol::Status::decode(&value).map_err(protocol_failure)?;
    Ok(Status {
        usb_enabled: status.usb_enabled,
        usb_ready: status.usb_ready,
        host_agent_present: status.host_agent_present,
        uptime_secs: Some(status.uptime_secs),
    })
}

fn protocol_failure(error: ble_protocol::Error) -> Failure {
    match error {
        ble_protocol::Error::Version => Failure::Incompatible,
        _ => Failure::InvalidData,
    }
}

fn failure(error: btleplug::Error) -> Failure {
    match error {
        btleplug::Error::PermissionDenied => Failure::PermissionDenied,
        btleplug::Error::NoAdapterAvailable => Failure::AdapterUnavailable,
        btleplug::Error::NotConnected | btleplug::Error::DeviceNotFound => Failure::LinkLost,
        btleplug::Error::TimedOut(_) => Failure::Timeout,
        _ => Failure::BluetoothUnavailable,
    }
}

async fn run(
    mut radio: impl RadioIo,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    mut reset: watch::Receiver<u64>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        let command = tokio::select! {
            biased;
            _ = &mut stop => break,
            changed = reset.changed() => {
                if changed.is_err() { break; }
                reset.borrow_and_update();
                radio.cleanup().await;
                continue;
            }
            command = commands.recv() => match command { Some(command) => command, None => break },
        };
        if command.generation != *reset.borrow() {
            continue;
        }
        let tag = command.request.tag;
        let result = tokio::select! {
            biased;
            _ = &mut stop => break,
            changed = reset.changed() => {
                if changed.is_err() { break; }
                reset.borrow_and_update();
                radio.cleanup().await;
                continue;
            }
            result = tokio::time::timeout(IO_TIMEOUT, radio.request(command.request.operation)) =>
                result.unwrap_or(Err(Failure::Timeout)),
        };
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                radio.cleanup().await;
                Reply::Error(error)
            }
        };
        if events.try_send(Event::Reply { tag, reply }).is_err() {
            break;
        }
    }
    radio.cleanup().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use companion_core::Tag;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct StalledRadio(Arc<AtomicUsize>);
    impl RadioIo for StalledRadio {
        async fn cleanup(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        async fn request(&mut self, operation: Operation) -> Result<Reply, Failure> {
            match operation {
                Operation::Scan => std::future::pending().await,
                Operation::Status => Ok(Reply::Status(Status::default())),
                _ => Err(Failure::ReadOnly),
            }
        }
    }

    #[tokio::test]
    async fn reset_interrupts_io_and_discards_queued_work_from_old_connection() {
        let cleanups = Arc::new(AtomicUsize::new(0));
        let mut transport = BleTransport::spawn(StalledRadio(cleanups.clone()));
        let old = Tag {
            epoch: 1,
            request: 1,
        };
        transport
            .submit(
                Request {
                    tag: old,
                    operation: Operation::Scan,
                },
                Duration::ZERO,
            )
            .unwrap();
        tokio::task::yield_now().await;
        transport
            .submit(
                Request {
                    tag: old,
                    operation: Operation::Status,
                },
                Duration::ZERO,
            )
            .unwrap();
        transport.close();
        let current = Tag {
            epoch: 2,
            request: 2,
        };
        transport
            .submit(
                Request {
                    tag: current,
                    operation: Operation::Status,
                },
                Duration::ZERO,
            )
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), transport.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, Event::Reply { tag, reply: Reply::Status(_) } if tag == current));
        assert!(transport.poll(Duration::ZERO).is_none());
        assert!(cleanups.load(Ordering::Relaxed) >= 1);
        transport.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_interrupts_stalled_io() {
        let cleanups = Arc::new(AtomicUsize::new(0));
        let mut transport = BleTransport::spawn(StalledRadio(cleanups.clone()));
        transport
            .submit(
                Request {
                    tag: Tag {
                        epoch: 1,
                        request: 1,
                    },
                    operation: Operation::Scan,
                },
                Duration::ZERO,
            )
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::timeout(Duration::from_secs(1), transport.shutdown())
            .await
            .unwrap();
        assert_eq!(cleanups.load(Ordering::Relaxed), 1);
    }
}

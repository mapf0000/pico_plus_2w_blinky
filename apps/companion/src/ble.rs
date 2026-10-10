//! Bounded CoreBluetooth/BlueZ/WinRT control worker.
use btleplug::{
    api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType},
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
const AUTH: Uuid = Uuid::from_u128(ble_protocol::AUTH_UUID);
const COMMAND: Uuid = Uuid::from_u128(ble_protocol::COMMAND_UUID);
const RESULT: Uuid = Uuid::from_u128(ble_protocol::RESULT_UUID);
const STATUS: Uuid = Uuid::from_u128(ble_protocol::STATUS_UUID);
const _: () = assert!(ble_protocol::MAX_MESSAGE_LEN == ble_session::MAX_CIPHERTEXT);
const _: () = assert!(ble_protocol::VERSION == 3 && ble_protocol::KIND_RECORD == 4);

#[derive(Clone)]
struct Command {
    generation: u64,
    request: Request,
}

pub struct BleTransport {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    reset: watch::Sender<u64>,
    cancel: watch::Sender<Option<Command>>,
    stop: Option<oneshot::Sender<()>>,
    worker: JoinHandle<()>,
}

impl BleTransport {
    pub fn new(profile: Option<ble_session::provisioning::Profile>) -> Self {
        Self::spawn(Radio {
            profile,
            ..Radio::default()
        })
    }

    fn spawn(radio: impl RadioIo + Send + 'static) -> Self {
        let (commands, rx) = mpsc::channel(4);
        let (tx, events) = mpsc::channel(16);
        let (reset, reset_rx) = watch::channel(0);
        let (cancel, cancel_rx) = watch::channel(None);
        let (stop, stop_rx) = oneshot::channel();
        let worker = tokio::spawn(run(radio, rx, tx, reset_rx, cancel_rx, stop_rx));
        Self {
            commands,
            events,
            reset,
            cancel,
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
        if matches!(request.operation, Operation::Cancel(_)) {
            self.cancel.send_replace(Some(Command {
                generation: *self.reset.borrow(),
                request,
            }));
            return Ok(());
        }
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
    profile: Option<ble_session::provisioning::Profile>,
    crypto: Option<ble_session::Session>,
    adapter: Option<Adapter>,
    devices: Vec<Device>,
    connected: Option<Peripheral>,
    scanning: bool,
    token: u16,
    active: Option<(script_protocol::EffectId, u16)>,
}

trait RadioIo {
    fn cleanup(&mut self) -> impl std::future::Future<Output = ()> + Send;
    fn request(
        &mut self,
        request: Request,
        events: &mpsc::Sender<Event>,
    ) -> impl std::future::Future<Output = Result<Reply, Failure>> + Send;
}

impl RadioIo for Radio {
    async fn cleanup(&mut self) {
        self.active = None;
        self.crypto = None;
        self.token = 0;
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

    async fn request(
        &mut self,
        request: Request,
        events: &mpsc::Sender<Event>,
    ) -> Result<Reply, Failure> {
        let tag = request.tag;
        match request.operation {
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
                let status = if self.profile.is_some() {
                    self.authenticate().await?;
                    self.snapshot().await?.1
                } else {
                    read_status(&peripheral).await?
                };
                Ok(Reply::Connected(Capabilities {
                    read_only: false,
                    script_version: script_protocol::VERSION,
                    upload_timeout: Duration::from_secs(60),
                    firmware: info.build_label().into(),
                    layouts: companion_core::layouts()
                        .iter()
                        .map(|layout| (*layout).into())
                        .collect(),
                    status,
                }))
            }
            Operation::Status => {
                let peripheral = self.connected.as_ref().ok_or(Failure::LinkLost)?;
                if !peripheral.is_connected().await.map_err(failure)? {
                    return Err(Failure::LinkLost);
                }
                if self.crypto.is_some() {
                    let (_, status) = self.snapshot().await?;
                    Ok(Reply::Status(status))
                } else {
                    Ok(Reply::Status(read_status(peripheral).await?))
                }
            }
            Operation::Acquire => {
                if self.crypto.is_none() {
                    self.authenticate().await?;
                }
                let code = self.control(ble_protocol::ACQUIRE).await?;
                if code == ble_protocol::Code::Acquired {
                    Ok(Reply::Acquired)
                } else {
                    Err(code_failure(code))
                }
            }
            Operation::Release => {
                let code = self.control(ble_protocol::RELEASE).await?;
                if code == ble_protocol::Code::Released {
                    Ok(Reply::Released)
                } else {
                    Err(code_failure(code))
                }
            }
            Operation::SetUsbEnabled(enabled) => {
                let code = self
                    .control(if enabled {
                        ble_protocol::USB_ON
                    } else {
                        ble_protocol::USB_OFF
                    })
                    .await?;
                if code != ble_protocol::Code::UsbChanged {
                    return Err(code_failure(code));
                }
                let (_, status) = self.snapshot().await?;
                Ok(Reply::Status(status))
            }
            Operation::Run(frame) => {
                let script_protocol::Message::Run { id, .. } =
                    script_protocol::decode(&frame).map_err(|_| Failure::InvalidData)?
                else {
                    return Err(Failure::InvalidData);
                };
                let token = self.next_token()?;
                self.active = Some((id, token));
                self.upload(ble_protocol::KIND_SCRIPT, token, &frame)
                    .await?;
                let first = self.wait_result(token, None, false).await?;
                if first == ble_protocol::Code::Accepted {
                    events
                        .try_send(Event::Reply {
                            tag,
                            reply: Reply::Accepted(id),
                        })
                        .map_err(|_| Failure::QueueFull)?;
                    let code = self.wait_result(token, None, true).await?;
                    self.active = None;
                    finished(id, code)
                } else {
                    self.active = None;
                    finished(id, first)
                }
            }
            Operation::Cancel(frame) => {
                let script_protocol::Message::Cancel { id } =
                    script_protocol::decode(&frame).map_err(|_| Failure::InvalidData)?
                else {
                    return Err(Failure::InvalidData);
                };
                let Some((expected, run_token)) = self.active else {
                    // Priority cancellation may arrive before its queued Run starts.
                    // The actor discards that Run; no device command was admitted.
                    return Ok(Reply::Finished {
                        id,
                        outcome: companion_core::Outcome::Cancelled,
                    });
                };
                if id != expected {
                    return Err(Failure::InvalidData);
                }
                let token = self.next_token()?;
                self.upload(ble_protocol::KIND_SCRIPT, token, &frame)
                    .await?;
                let code = self.wait_result(run_token, Some(token), true).await?;
                self.active = None;
                finished(id, code)
            }
        }
    }
}

impl Radio {
    fn next_token(&mut self) -> Result<u16, Failure> {
        self.token = self.token.checked_add(1).ok_or(Failure::IdExhausted)?;
        Ok(self.token)
    }
    async fn authenticate(&mut self) -> Result<(), Failure> {
        let profile = self.profile.as_ref().ok_or(Failure::MissingKey)?;
        let mut ephemeral = [0; 32];
        getrandom::fill(&mut ephemeral).map_err(|_| Failure::Authentication)?;
        let mut handshake = ble_session::Handshake::new(true, profile, ephemeral);
        zeroize::Zeroize::zeroize(&mut ephemeral);
        let mut message = [0; ble_session::HANDSHAKE_LEN];
        handshake
            .write(&mut message)
            .map_err(|_| Failure::Authentication)?;
        let token = self.next_token()?;
        self.write_fragments(AUTH, ble_protocol::KIND_HANDSHAKE, token, &message)
            .await
            .map_err(authentication_io_failure)?;
        let (kind, received_token, bytes) = self
            .read_response()
            .await
            .map_err(authentication_io_failure)?;
        if kind != ble_protocol::KIND_HANDSHAKE || received_token != token {
            return Err(Failure::Authentication);
        }
        handshake
            .read(&bytes)
            .map_err(|_| Failure::Authentication)?;
        self.crypto = Some(handshake.finish().map_err(|_| Failure::Authentication)?);
        Ok(())
    }
    async fn upload(&mut self, kind: u8, token: u16, bytes: &[u8]) -> Result<(), Failure> {
        use zeroize::Zeroize;
        if bytes.len() + 1 > ble_session::MAX_PLAINTEXT {
            return Err(Failure::InvalidData);
        }
        let mut plaintext = zeroize::Zeroizing::new(vec![kind]);
        plaintext.extend_from_slice(bytes);
        let mut encrypted = vec![0; plaintext.len() + ble_session::TAG_LEN];
        let result = self.crypto.as_mut().ok_or(Failure::Authentication)?.seal(
            token,
            &plaintext,
            &mut encrypted,
        );
        plaintext.zeroize();
        let length = result.map_err(|_| Failure::Authentication)?;
        self.write_fragments(
            COMMAND,
            ble_protocol::KIND_RECORD,
            token,
            &encrypted[..length],
        )
        .await
    }
    async fn write_fragments(
        &self,
        uuid: Uuid,
        kind: u8,
        token: u16,
        bytes: &[u8],
    ) -> Result<(), Failure> {
        let peripheral = self.connected.as_ref().ok_or(Failure::LinkLost)?;
        let characteristic = peripheral
            .characteristics()
            .into_iter()
            .find(|c| {
                c.service_uuid == SERVICE
                    && c.uuid == uuid
                    && c.properties.contains(CharPropFlags::WRITE)
            })
            .ok_or(Failure::Incompatible)?;
        for offset in (0..bytes.len()).step_by(ble_protocol::PAYLOAD_LEN) {
            let (frame, length) =
                ble_protocol::fragment(kind, token, bytes, offset).map_err(protocol_failure)?;
            peripheral
                .write(&characteristic, &frame[..length], WriteType::WithResponse)
                .await
                .map_err(failure)?;
        }
        Ok(())
    }
    async fn read_response(&self) -> Result<(u8, u16, Vec<u8>), Failure> {
        let peripheral = self.connected.as_ref().ok_or(Failure::LinkLost)?;
        let characteristic = characteristic(peripheral, RESULT)?;
        let mut receiver = ble_protocol::Receiver::new();
        // At most four default-MTU reads for a handshake or encrypted snapshot.
        for _ in 0..ble_protocol::MAX_RESPONSE_LEN.div_ceil(ble_protocol::PAYLOAD_LEN) {
            let bytes = peripheral.read(&characteristic).await.map_err(failure)?;
            let frame = ble_protocol::read_fragment(&bytes).map_err(protocol_failure)?;
            let total = u16::from_le_bytes([frame[6], frame[7]]) as usize;
            if total > ble_protocol::MAX_RESPONSE_LEN {
                return Err(Failure::InvalidData);
            }
            if let Some(message) = receiver.push(frame).map_err(protocol_failure)? {
                return Ok((message.kind, message.token, message.payload.to_vec()));
            }
        }
        Err(Failure::InvalidData)
    }
    async fn snapshot(&mut self) -> Result<(ble_protocol::ResultValue, Status), Failure> {
        let token = self.next_token()?;
        self.upload(ble_protocol::KIND_CONTROL, token, &[ble_protocol::POLL])
            .await?;
        let (kind, sequence, encrypted) = self.read_response().await?;
        if kind != ble_protocol::KIND_RECORD {
            return Err(Failure::Authentication);
        }
        let mut plaintext = zeroize::Zeroizing::new([0; ble_protocol::SNAPSHOT_LEN]);
        let length = self
            .crypto
            .as_mut()
            .ok_or(Failure::Authentication)?
            .open(sequence, &encrypted, &mut *plaintext)
            .map_err(|_| Failure::Authentication)?;
        if length != ble_protocol::SNAPSHOT_LEN {
            return Err(Failure::InvalidData);
        }
        let result =
            ble_protocol::ResultValue::decode(&plaintext[..20]).map_err(protocol_failure)?;
        let status = ble_protocol::Status::decode(&plaintext[20..]).map_err(protocol_failure)?;
        Ok((result, status_model(status)))
    }
    async fn wait_result(
        &mut self,
        token: u16,
        alternate: Option<u16>,
        terminal: bool,
    ) -> Result<ble_protocol::Code, Failure> {
        loop {
            let (value, _) = self.snapshot().await?;
            if (value.token == token || alternate == Some(value.token))
                && (!terminal || value.code != ble_protocol::Code::Accepted)
            {
                return Ok(value.code);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn control(&mut self, opcode: u8) -> Result<ble_protocol::Code, Failure> {
        let token = self.next_token()?;
        self.upload(ble_protocol::KIND_CONTROL, token, &[opcode])
            .await?;
        self.wait_result(token, None, false).await
    }
}
fn code_failure(code: ble_protocol::Code) -> Failure {
    match code {
        ble_protocol::Code::Busy => Failure::Busy,
        ble_protocol::Code::UsbUnavailable => Failure::UsbUnavailable,
        _ => Failure::NotReady,
    }
}
fn finished(id: script_protocol::EffectId, code: ble_protocol::Code) -> Result<Reply, Failure> {
    use ble_protocol::Code;
    let outcome = match code {
        Code::Completed => companion_core::Outcome::Completed,
        Code::Cancelled => companion_core::Outcome::Cancelled,
        Code::Rejected => companion_core::Outcome::Rejected,
        Code::UsbUnavailable => companion_core::Outcome::UsbUnavailable,
        _ => return Err(code_failure(code)),
    };
    Ok(Reply::Finished { id, outcome })
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
    Ok(status_model(status))
}

fn status_model(status: ble_protocol::Status) -> Status {
    Status {
        usb_enabled: status.usb_enabled,
        usb_ready: status.usb_ready,
        host_agent_present: status.host_agent_present,
        uptime_secs: Some(status.uptime_secs),
    }
}

fn protocol_failure(error: ble_protocol::Error) -> Failure {
    match error {
        ble_protocol::Error::Version => Failure::Incompatible,
        _ => Failure::InvalidData,
    }
}

fn authentication_io_failure(error: Failure) -> Failure {
    // A peer with a different key closes the connection instead of emitting an
    // unauthenticated credential oracle. Preserve permission/timeout errors.
    match error {
        Failure::LinkLost | Failure::BluetoothUnavailable | Failure::InvalidData => {
            Failure::Authentication
        }
        other => other,
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
    mut cancel: watch::Receiver<Option<Command>>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut urgent = None;
    let mut cancelled_tag = None;
    loop {
        let command = if let Some(command) = urgent.take() {
            command
        } else {
            tokio::select! {
                biased;
                _ = &mut stop => break,
                changed = reset.changed() => {
                    if changed.is_err() { break; }
                    reset.borrow_and_update();
                    radio.cleanup().await;
                    continue;
                }
                changed = cancel.changed() => {
                    if changed.is_err() { break; }
                    urgent = cancel.borrow_and_update().clone(); continue;
                }
                command = commands.recv() => match command { Some(command) => command, None => break },
            }
        };
        if command.generation != *reset.borrow() {
            continue;
        }
        let tag = command.request.tag;
        if matches!(command.request.operation, Operation::Cancel(_)) {
            cancelled_tag = Some(tag);
        } else if matches!(command.request.operation, Operation::Run(_))
            && cancelled_tag == Some(tag)
        {
            // An urgent Cancel can overtake an unstarted Run in the normal queue.
            continue;
        }
        let timeout = match &command.request.operation {
            Operation::Acquire | Operation::Connect(_) => Duration::from_secs(10),
            Operation::Run(_) => Duration::from_secs(300),
            _ => IO_TIMEOUT,
        };
        let result = tokio::select! {
            biased;
            _ = &mut stop => break,
            changed = reset.changed() => {
                if changed.is_err() { break; }
                reset.borrow_and_update();
                radio.cleanup().await;
                continue;
            }
            changed = cancel.changed() => {
                if changed.is_err() { break; }
                urgent = cancel.borrow_and_update().clone(); continue;
            }
            result = tokio::time::timeout(timeout, radio.request(command.request,&events)) =>
                result.unwrap_or(Err(Failure::Timeout)),
        };
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                if !matches!(
                    error,
                    Failure::Busy | Failure::UsbUnavailable | Failure::NotReady
                ) {
                    radio.cleanup().await;
                }
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
        async fn request(
            &mut self,
            request: Request,
            _: &mpsc::Sender<Event>,
        ) -> Result<Reply, Failure> {
            match request.operation {
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
    #[tokio::test]
    async fn cancel_interrupts_a_stalled_upload_without_resetting_the_connection() {
        struct UploadRadio(Arc<AtomicUsize>);
        impl RadioIo for UploadRadio {
            async fn cleanup(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            async fn request(
                &mut self,
                request: Request,
                _: &mpsc::Sender<Event>,
            ) -> Result<Reply, Failure> {
                match request.operation {
                    Operation::Run(_) => std::future::pending().await,
                    Operation::Cancel(_) => Ok(Reply::Finished {
                        id: script_protocol::EffectId {
                            request_id: 1,
                            process_id: 1,
                            effect_id: 1,
                        },
                        outcome: companion_core::Outcome::Cancelled,
                    }),
                    _ => Err(Failure::InvalidData),
                }
            }
        }
        let cleanups = Arc::new(AtomicUsize::new(0));
        let mut transport = BleTransport::spawn(UploadRadio(cleanups.clone()));
        let tag = Tag {
            epoch: 1,
            request: 1,
        };
        transport
            .submit(
                Request {
                    tag,
                    operation: Operation::Run(vec![]),
                },
                Duration::ZERO,
            )
            .unwrap();
        tokio::task::yield_now().await;
        transport
            .submit(
                Request {
                    tag,
                    operation: Operation::Cancel(vec![]),
                },
                Duration::ZERO,
            )
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), transport.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event,Event::Reply { tag:other,reply:Reply::Finished { outcome:companion_core::Outcome::Cancelled,.. }} if other == tag)
        );
        assert_eq!(cleanups.load(Ordering::Relaxed), 0);
        transport.shutdown().await;
    }
    #[tokio::test]
    async fn cancel_before_queued_run_starts_discards_the_run() {
        struct QueuedRadio(Arc<AtomicUsize>);
        impl RadioIo for QueuedRadio {
            async fn cleanup(&mut self) {}
            async fn request(
                &mut self,
                request: Request,
                _: &mpsc::Sender<Event>,
            ) -> Result<Reply, Failure> {
                match request.operation {
                    Operation::Run(_) => {
                        self.0.fetch_add(1, Ordering::Relaxed);
                        Err(Failure::InvalidData)
                    }
                    Operation::Cancel(_) => Ok(Reply::Finished {
                        id: script_protocol::EffectId {
                            request_id: 1,
                            process_id: 1,
                            effect_id: 1,
                        },
                        outcome: companion_core::Outcome::Cancelled,
                    }),
                    Operation::Status => Ok(Reply::Status(Status::default())),
                    _ => Err(Failure::InvalidData),
                }
            }
        }
        let started = Arc::new(AtomicUsize::new(0));
        let mut transport = BleTransport::spawn(QueuedRadio(started.clone()));
        let tag = Tag {
            epoch: 1,
            request: 1,
        };
        // No yield: both requests are queued before the actor runs.
        transport
            .submit(
                Request {
                    tag,
                    operation: Operation::Run(vec![]),
                },
                Duration::ZERO,
            )
            .unwrap();
        transport
            .submit(
                Request {
                    tag,
                    operation: Operation::Cancel(vec![]),
                },
                Duration::ZERO,
            )
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), transport.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            Event::Reply {
                reply: Reply::Finished {
                    outcome: companion_core::Outcome::Cancelled,
                    ..
                },
                ..
            }
        ));
        // A following read proves the queue has been drained through the skipped Run.
        let next = Tag {
            epoch: 1,
            request: 2,
        };
        transport
            .submit(
                Request {
                    tag: next,
                    operation: Operation::Status,
                },
                Duration::ZERO,
            )
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), transport.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event,Event::Reply { tag:other,reply:Reply::Status(_) } if other == next));
        assert_eq!(started.load(Ordering::Relaxed), 0);
        transport.shutdown().await;
    }
}

//! Bounded CoreBluetooth/BlueZ/WinRT control worker.
use btleplug::{
    api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use companion_core::{
    Capabilities, Device, Event, Failure, MAX_DEVICES, Operation, Reply, Request, Status, Transport,
};
use futures_util::{FutureExt, StreamExt};
use std::pin::Pin;
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
const EVENTS: Uuid = Uuid::from_u128(ble_protocol::EVENTS_UUID);
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
        if matches!(
            request.operation,
            Operation::Cancel(_) | Operation::CancelDirectory
        ) {
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

struct Radio {
    notifications:
        Option<Pin<Box<dyn futures_util::Stream<Item = btleplug::api::ValueNotification> + Send>>>,
    receiver: ble_protocol::Receiver,
    fragments: u8,
    ack: Option<(u16, u16)>,
    streams: bool,
    epoch: u64,
    events: Option<mpsc::Sender<Event>>,
    profile: Option<ble_session::provisioning::Profile>,
    crypto: Option<ble_session::Session>,
    adapter: Option<Adapter>,
    devices: Vec<Device>,
    connected: Option<Peripheral>,
    scanning: bool,
    token: u16,
    active: Option<(script_protocol::EffectId, u16)>,
}

impl Default for Radio {
    fn default() -> Self {
        Self {
            notifications: None,
            receiver: ble_protocol::Receiver::new_stream(),
            fragments: 0,
            ack: None,
            streams: false,
            epoch: 0,
            events: None,
            profile: None,
            crypto: None,
            adapter: None,
            devices: Vec::new(),
            connected: None,
            scanning: false,
            token: 0,
            active: None,
        }
    }
}
trait RadioIo {
    fn idle(
        &mut self,
        _: &mpsc::Sender<Event>,
    ) -> impl std::future::Future<Output = Result<(), Failure>> + Send {
        std::future::pending()
    }

    fn cleanup(&mut self) -> impl std::future::Future<Output = ()> + Send;
    fn request(
        &mut self,
        request: Request,
        events: &mpsc::Sender<Event>,
    ) -> impl std::future::Future<Output = Result<Reply, Failure>> + Send;
}

impl RadioIo for Radio {
    async fn idle(&mut self, events: &mpsc::Sender<Event>) -> Result<(), Failure> {
        match self.next_stream(events).await {
            Ok(_) => Ok(()),
            Err(error) => {
                let _ = events.try_send(Event::Lost { epoch: self.epoch });
                Err(error)
            }
        }
    }

    async fn cleanup(&mut self) {
        self.active = None;
        self.notifications = None;
        self.receiver = ble_protocol::Receiver::new_stream();
        self.fragments = 0;
        self.ack = None;
        self.streams = false;
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
        self.events = Some(events.clone());
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
                self.epoch = tag.epoch;
                self.streams = info.streams;
                if info.streams {
                    let events_char = peripheral
                        .characteristics()
                        .into_iter()
                        .find(|c| {
                            c.service_uuid == SERVICE
                                && c.uuid == EVENTS
                                && c.properties.contains(CharPropFlags::NOTIFY)
                        })
                        .ok_or(Failure::Incompatible)?;
                    self.notifications = Some(peripheral.notifications().await.map_err(failure)?);
                    peripheral.subscribe(&events_char).await.map_err(failure)?;
                }

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
                    overview_supported: info.overview,
                    overview: None,
                    identity_supported: info.identity,
                    filesystem_supported: info.streams,
                    identity: None,
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
                    if self.streams {
                        self.flow(ble_protocol::stream::Flow::Start).await?;
                    }
                    Ok(Reply::Acquired)
                } else {
                    Err(code_failure(code))
                }
            }
            Operation::Overview => {
                let token = self.next_token()?;
                self.upload(
                    ble_protocol::KIND_CONTROL,
                    token,
                    &[ble_protocol::READ_OVERVIEW],
                )
                .await?;
                let plaintext = self.protected_response().await?;
                let value = ble_protocol::Overview::decode(&plaintext).map_err(protocol_failure)?;
                if value.request != token {
                    return Err(Failure::InvalidData);
                }
                Ok(Reply::Overview(companion_core::Overview::from_bytes(
                    &plaintext,
                )?))
            }
            Operation::Release => {
                let code = self.control(ble_protocol::RELEASE).await?;
                if code == ble_protocol::Code::Released {
                    Ok(Reply::Released)
                } else {
                    Err(code_failure(code))
                }
            }
            Operation::ListDirectory {
                path,
                cursor,
                hidden,
            } => {
                if !self.streams {
                    return Err(Failure::Incompatible);
                }
                let mut body = [0; 2 + 19 + ble_protocol::filesystem::PATH_MAX];
                body[0] = 1;
                body[1] = 0;
                let n = ble_protocol::filesystem::Request {
                    id: tag.request,
                    cursor,
                    hidden,
                    path: &path,
                }
                .encode(&mut body[2..])
                .map_err(protocol_failure)?;
                self.filesystem_command(&body[..n + 2], ble_protocol::Code::DirectoryAccepted)
                    .await?;
                loop {
                    let (kind, body) = self.next_record(events).await?;
                    if kind == ble_protocol::stream::FILESYSTEM
                        && ble_protocol::filesystem::Page::decode(&body)
                            .map_err(protocol_failure)?
                            .id
                            == tag.request
                    {
                        return Ok(Reply::Directory(body));
                    }
                }
            }
            Operation::CancelDirectory => {
                let mut body = [1, 1, 0, 0, 0, 0, 0, 0, 0, 0];
                body[2..].copy_from_slice(&tag.request.to_le_bytes());
                self.filesystem_command(&body, ble_protocol::Code::DirectoryCancelled)
                    .await?;
                Ok(Reply::DirectoryCancelled)
            }
            Operation::StreamProbe => {
                let started = std::time::Instant::now();
                self.flow(ble_protocol::stream::Flow::Probe).await?;
                loop {
                    let (kind, body) = self.next_record(events).await?;
                    if kind == ble_protocol::stream::PROBE {
                        return Ok(Reply::StreamMeasured {
                            bytes: body.len(),
                            elapsed: started.elapsed(),
                            att_mtu: u16::from_le_bytes([body[2040], body[2041]]),
                            queue_high_water: [body[2042], body[2043], body[2044]],
                        });
                    }
                }
            }
            Operation::ReadIdentity => self.identity(ble_protocol::identity::Command::Read).await,
            Operation::SaveIdentity(identity) => {
                self.identity(ble_protocol::identity::Command::Save(identity.value()))
                    .await
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
    async fn flow(&mut self, flow: ble_protocol::stream::Flow) -> Result<(), Failure> {
        let (body, n) = flow.encode();
        let token = self.next_token()?;
        self.upload(ble_protocol::KIND_FLOW, token, &body[..n])
            .await
    }
    async fn flush_ack(&mut self) -> Result<(), Failure> {
        if let Some((sequence, offset)) = self.ack {
            self.flow(ble_protocol::stream::Flow::Ack { sequence, offset })
                .await?;
            self.ack = None;
        }
        Ok(())
    }
    async fn filesystem_command(
        &mut self,
        body: &[u8],
        expected: ble_protocol::Code,
    ) -> Result<(), Failure> {
        let token = self.next_token()?;
        self.upload(ble_protocol::KIND_FILESYSTEM, token, body)
            .await?;
        let plain = self.protected_response().await?;
        if plain.len() != ble_protocol::SNAPSHOT_LEN {
            return Err(Failure::InvalidData);
        }
        let value = ble_protocol::ResultValue::decode(&plain[..20]).map_err(protocol_failure)?;
        if value.token != token {
            return Err(Failure::InvalidData);
        }
        if value.code == expected {
            Ok(())
        } else {
            Err(code_failure(value.code))
        }
    }
    async fn next_stream(
        &mut self,
        events: &mpsc::Sender<Event>,
    ) -> Result<Option<(u8, Vec<u8>)>, Failure> {
        self.flush_ack().await?;
        let Some(notifications) = self.notifications.as_mut() else {
            return std::future::pending().await;
        };
        let packet = notifications.next().await.ok_or(Failure::LinkLost)?;
        self.consume_notification(packet, events).await
    }
    async fn next_record(
        &mut self,
        events: &mpsc::Sender<Event>,
    ) -> Result<(u8, Vec<u8>), Failure> {
        loop {
            if let Some(record) = self.next_stream(events).await? {
                return Ok(record);
            }
        }
    }
    async fn consume_notification(
        &mut self,
        packet: btleplug::api::ValueNotification,
        events: &mpsc::Sender<Event>,
    ) -> Result<Option<(u8, Vec<u8>)>, Failure> {
        if packet.uuid != EVENTS {
            return Ok(None);
        }
        let frame = ble_protocol::read_fragment(&packet.value).map_err(protocol_failure)?;
        let sequence = u16::from_le_bytes([frame[2], frame[3]]);
        let offset = u16::from_le_bytes([frame[4], frame[5]]) as usize + frame.len() - 8;
        let complete = self.receiver.push(frame).map_err(protocol_failure)?;
        self.fragments += 1;
        let record = if let Some(message) = complete {
            let mut plain = zeroize::Zeroizing::new(vec![0; ble_protocol::stream::PLAINTEXT_MAX]);
            let n = self
                .crypto
                .as_mut()
                .ok_or(Failure::Authentication)?
                .open_stream(message.token, message.payload, &mut plain)
                .map_err(|_| Failure::Authentication)?;
            let (kind, body) =
                ble_protocol::stream::decode(&plain[..n]).map_err(protocol_failure)?;
            if kind == ble_protocol::stream::BUTTON {
                events
                    .try_send(Event::Button {
                        epoch: self.epoch,
                        button: body[0],
                    })
                    .map_err(|_| Failure::QueueFull)?;
            }
            Some((kind, body.to_vec()))
        } else {
            None
        };
        // ACK follows validation and admission into bounded application-owned storage.
        if self.fragments == ble_protocol::stream::WINDOW || record.is_some() {
            self.ack = Some((sequence, offset as u16));
            self.fragments = 0;
            self.flush_ack().await?;
        }
        Ok(record)
    }
    async fn pump_available(&mut self) -> Result<(), Failure> {
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        self.flush_ack().await?;
        for _ in 0..8 {
            let packet = self
                .notifications
                .as_mut()
                .and_then(|s| s.next().now_or_never())
                .flatten();
            let Some(packet) = packet else {
                break;
            };
            let _ = self.consume_notification(packet, &events).await?;
        }
        Ok(())
    }
    async fn identity(
        &mut self,
        command: ble_protocol::identity::Command<'_>,
    ) -> Result<Reply, Failure> {
        let mut body = [0; ble_protocol::identity::COMMAND_MAX];
        let length = command.encode(&mut body).map_err(protocol_failure)?;
        let token = self.next_token()?;
        self.upload(ble_protocol::KIND_IDENTITY, token, &body[..length])
            .await?;
        let plaintext = self.protected_response().await?;
        identity_reply(token, &plaintext)
    }
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
        &mut self,
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
    async fn read_response(&mut self) -> Result<(u8, u16, Vec<u8>), Failure> {
        let peripheral = self.connected.as_ref().ok_or(Failure::LinkLost)?;
        let characteristic = characteristic(peripheral, RESULT)?;
        let mut receiver = ble_protocol::Receiver::new();
        // Finite metadata is at most 26 default-MTU reads; snapshots still use four.
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
        let plaintext = self.protected_response().await?;
        if plaintext.len() != ble_protocol::SNAPSHOT_LEN {
            return Err(Failure::InvalidData);
        }
        let result =
            ble_protocol::ResultValue::decode(&plaintext[..20]).map_err(protocol_failure)?;
        let status = ble_protocol::Status::decode(&plaintext[20..]).map_err(protocol_failure)?;
        Ok((result, status_model(status)))
    }
    async fn protected_response(&mut self) -> Result<zeroize::Zeroizing<Vec<u8>>, Failure> {
        let (kind, sequence, encrypted) = self.read_response().await?;
        if kind != ble_protocol::KIND_RECORD {
            return Err(Failure::Authentication);
        }
        let mut plaintext = zeroize::Zeroizing::new(vec![0; ble_protocol::MAX_OVERVIEW_LEN]);
        let length = self
            .crypto
            .as_mut()
            .ok_or(Failure::Authentication)?
            .open(sequence, &encrypted, &mut plaintext)
            .map_err(|_| Failure::Authentication)?;
        plaintext.truncate(length);
        Ok(plaintext)
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
            self.pump_available().await?;
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
        ble_protocol::Code::AgentUnavailable => Failure::AgentUnavailable,
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
        btleplug::Error::RuntimeError(message) if message == "Device disconnected" => {
            Failure::LinkLost
        }
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
                result=radio.idle(&events) => { if result.is_err(){radio.cleanup().await;} continue; },
            }
        };
        if command.generation != *reset.borrow() {
            continue;
        }
        let tag = command.request.tag;
        if matches!(
            command.request.operation,
            Operation::Cancel(_) | Operation::CancelDirectory
        ) {
            cancelled_tag = Some(tag);
        } else if matches!(
            command.request.operation,
            Operation::Run(_) | Operation::ListDirectory { .. }
        ) && cancelled_tag == Some(tag)
        {
            // An urgent Cancel can overtake an unstarted Run in the normal queue.
            continue;
        }
        let timeout = match &command.request.operation {
            Operation::Acquire | Operation::Connect(_) => Duration::from_secs(10),
            Operation::Overview => Duration::from_secs(8),
            Operation::Run(_) => Duration::from_secs(300),
            Operation::ListDirectory { .. } | Operation::StreamProbe => Duration::from_secs(30),
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
                    Failure::Busy
                        | Failure::UsbUnavailable
                        | Failure::NotReady
                        | Failure::AgentUnavailable
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

    #[tokio::test]
    async fn directory_cancel_overtakes_queued_or_active_list_and_preserves_connection() {
        struct DirectoryRadio {
            started: Arc<AtomicUsize>,
            cleanups: Arc<AtomicUsize>,
        }
        impl RadioIo for DirectoryRadio {
            async fn cleanup(&mut self) {
                self.cleanups.fetch_add(1, Ordering::Relaxed);
            }
            async fn request(
                &mut self,
                request: Request,
                _: &mpsc::Sender<Event>,
            ) -> Result<Reply, Failure> {
                match request.operation {
                    Operation::ListDirectory { .. } => {
                        self.started.fetch_add(1, Ordering::Relaxed);
                        std::future::pending().await
                    }
                    Operation::CancelDirectory => Ok(Reply::DirectoryCancelled),
                    Operation::Status => Ok(Reply::Status(Status::default())),
                    _ => Err(Failure::InvalidData),
                }
            }
        }
        for active in [false, true] {
            let started = Arc::new(AtomicUsize::new(0));
            let cleanups = Arc::new(AtomicUsize::new(0));
            let mut transport = BleTransport::spawn(DirectoryRadio {
                started: started.clone(),
                cleanups: cleanups.clone(),
            });
            let tag = Tag {
                epoch: 1,
                request: 1,
            };
            transport
                .submit(
                    Request {
                        tag,
                        operation: Operation::ListDirectory {
                            path: String::new(),
                            cursor: 0,
                            hidden: false,
                        },
                    },
                    Duration::ZERO,
                )
                .unwrap();
            if active {
                tokio::time::timeout(Duration::from_secs(1), async {
                    while started.load(Ordering::Relaxed) == 0 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            transport
                .submit(
                    Request {
                        tag,
                        operation: Operation::CancelDirectory,
                    },
                    Duration::ZERO,
                )
                .unwrap();
            let event = tokio::time::timeout(Duration::from_secs(1), transport.events.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(event, Event::Reply { tag: other, reply: Reply::DirectoryCancelled } if other == tag)
            );
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
            assert!(
                matches!(event, Event::Reply { tag: other, reply: Reply::Status(_) } if other == next)
            );
            assert_eq!(started.load(Ordering::Relaxed), usize::from(active));
            assert_eq!(cleanups.load(Ordering::Relaxed), 0);
            assert!(transport.poll(Duration::ZERO).is_none());
            transport.shutdown().await;
        }
    }
}

fn identity_reply(token: u16, plaintext: &[u8]) -> Result<Reply, Failure> {
    let response = ble_protocol::identity::Response::decode(plaintext).map_err(protocol_failure)?;
    if response.request != token {
        return Err(Failure::InvalidData);
    }
    use ble_protocol::identity::Status;
    let error = match response.status {
        Status::Success => None,
        Status::UsbActive => Some(Failure::UsbActive),
        Status::Busy => Some(Failure::Busy),
        Status::Invalid => Some(Failure::InvalidIdentity),
        Status::FlashFailure => Some(Failure::Persistence),
    };
    Ok(Reply::Identity {
        identity: companion_core::UsbIdentity::new(
            response.identity.manufacturer,
            response.identity.product,
        )?,
        error,
    })
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn identity_reply_requires_exact_correlation_and_preserves_flash_failure() {
        use ble_protocol::identity::{Identity, RESPONSE_MAX, Response, Status};
        let mut bytes = [0; RESPONSE_MAX];
        let response = Response {
            request: 42,
            status: Status::FlashFailure,
            identity: Identity {
                manufacturer: "Saved",
                product: "Current",
            },
        };
        let len = response.encode(&mut bytes).unwrap();
        assert!(matches!(
            identity_reply(41, &bytes[..len]),
            Err(Failure::InvalidData)
        ));
        assert!(matches!(
            identity_reply(42, &bytes[..len]),
            Ok(Reply::Identity {
                error: Some(Failure::Persistence),
                ..
            })
        ));
        assert!(matches!(
            identity_reply(42, &bytes[..len - 1]),
            Err(Failure::InvalidData)
        ));
    }
}

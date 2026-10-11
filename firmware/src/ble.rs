//! Bluetooth control authenticated by a provisioned Noise PSK, without pairing.
use ble_protocol::Code;
use embassy_futures::select::{Either, select};
use embassy_sync_ble as embassy_sync;
use embassy_time::{Duration, Instant, Timer};
use portable_atomic::Ordering;
use static_cell::StaticCell;
use trouble_host::prelude::*;
const _: () = assert!(ble_protocol::MAX_MESSAGE_LEN == ble_session::MAX_CIPHERTEXT);
const _: () = assert!(ble_protocol::VERSION == 3 && ble_protocol::KIND_RECORD == 4);
const _: () = assert!(ble_protocol::MAX_RESPONSE_LEN <= ble_session::MAX_CIPHERTEXT);
const _: () = assert!(ble_protocol::identity::RESPONSE_MAX <= ble_protocol::MAX_OVERVIEW_LEN);

const SERVICE_UUID: Uuid = Uuid::new_long(ble_protocol::SERVICE_UUID.to_le_bytes());
const INFO_UUID: Uuid = Uuid::new_long(ble_protocol::INFO_UUID.to_le_bytes());
const STATUS_UUID: Uuid = Uuid::new_long(ble_protocol::STATUS_UUID.to_le_bytes());
const AUTH_UUID: Uuid = Uuid::new_long(ble_protocol::AUTH_UUID.to_le_bytes());
const COMMAND_UUID: Uuid = Uuid::new_long(ble_protocol::COMMAND_UUID.to_le_bytes());
const RESULT_UUID: Uuid = Uuid::new_long(ble_protocol::RESULT_UUID.to_le_bytes());
const EVENTS_UUID: Uuid = Uuid::new_long(ble_protocol::EVENTS_UUID.to_le_bytes());
const NAME: &str = "Pico BLE";

#[gatt_server]
struct Server {
    pico: PicoService,
}
#[gatt_service(uuid = SERVICE_UUID)]
struct PicoService {
    #[characteristic(uuid = INFO_UUID, read)]
    info: [u8; 20],
    #[characteristic(uuid = STATUS_UUID, read)]
    status: [u8; 12],
    #[characteristic(uuid = AUTH_UUID, write)]
    auth: [u8; 20],
    #[characteristic(uuid = COMMAND_UUID, write)]
    command: [u8; 20],
    #[characteristic(uuid = RESULT_UUID, read)]
    result: [u8; 20],
    #[characteristic(uuid = EVENTS_UUID, notify)]
    events: [u8; 20],
}
fn status() -> [u8; ble_protocol::STATUS_LEN] {
    ble_protocol::Status {
        usb_enabled: crate::usb::usb_supervisor::USB_ENABLED.load(Ordering::SeqCst),
        usb_ready: crate::usb::hid::USB_READY.load(Ordering::SeqCst),
        host_agent_present: crate::capabilities::host_agent_present(),
        uptime_secs: Instant::now().as_secs(),
    }
    .encode()
}

fn overview(token: u16, out: &mut [u8]) -> Result<usize, ble_protocol::Error> {
    use crate::usb::bootstrap::Phase;
    let agent = crate::capabilities::host_agent_snapshot();
    ble_protocol::Overview {
        request: token,
        bootstrap: match crate::usb::bootstrap::phase() {
            Phase::Idle => ble_protocol::BootstrapPhase::Idle,
            Phase::Armed => ble_protocol::BootstrapPhase::Armed,
            Phase::Downloading => ble_protocol::BootstrapPhase::Downloading,
            Phase::Verified => ble_protocol::BootstrapPhase::Verified,
            Phase::Failed => ble_protocol::BootstrapPhase::Failed,
            Phase::Cancelled => ble_protocol::BootstrapPhase::Cancelled,
        },
        bootstrap_available: crate::usb::bootstrap::available(),
        script_version: script_protocol::VERSION,
        filesystem_version: crate::capabilities::FILESYSTEM_PROTOCOL_VERSION,
        transfer_version: transfer_protocol::TRANSFER_PROTOCOL_VERSION,
        max_bytecode: script_protocol::MAX_BYTECODE as u16,
        max_usb_payload: transfer_protocol::TLV_MAX_PAYLOAD as u16,
        agent_age_ms: crate::capabilities::host_agent_age_ms(),
        max_delay_ms: bytecode_constants::MAX_DEVICE_DELAY_MS,
        max_total_delay_ms: bytecode_constants::MAX_EFFECT_DELAY_TOTAL_MS as u32,
        firmware: env!("PICO_FIRMWARE_BUILD"),
        agent_version: &agent.version,
        host_os: &agent.host_os,
        hostname: &agent.hostname,
    }
    .encode(out)
}

#[embassy_executor::task]
pub async fn task(driver: cyw43::bluetooth::BtDriver<'static>, address: [u8; 6]) {
    // One connection, three channels, eight 128-byte packets, and no allocator.
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 3>> = StaticCell::new();
    let controller = ExternalController::<_, 10>::new(driver);
    let stack = trouble_host::new(controller, RESOURCES.init(HostResources::new()))
        .set_random_address(Address::random(address));
    let Host {
        mut peripheral,
        mut runner,
        ..
    } = stack.build();
    let profile = crate::device_config::control_profile().await;
    if profile.is_none() {
        log::warn!("ble: control key not provisioned; control unavailable");
    }
    let server = match Server::new_with_config(GapConfig::default(NAME)) {
        Ok(server) => server,
        Err(_) => {
            log::error!("ble: GATT initialization failed");
            return;
        }
    };
    if server
        .set(
            &server.pico.info,
            &ble_protocol::Info::new(env!("PICO_FIRMWARE_BUILD")).encode(),
        )
        .is_err()
    {
        return;
    }
    let uuids = [ble_protocol::SERVICE_UUID.to_le_bytes()];
    let mut ad = [0; 31];
    let Ok(ad_len) = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids128(&uuids),
        ],
        &mut ad,
    ) else {
        return;
    };
    let mut scan = [0; 31];
    let Ok(scan_len) = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(NAME.as_bytes())],
        &mut scan,
    ) else {
        return;
    };
    let service = async {
        loop {
            let advertiser = match peripheral
                .advertise(
                    &Default::default(),
                    Advertisement::ConnectableScannableUndirected {
                        adv_data: &ad[..ad_len],
                        scan_data: &scan[..scan_len],
                    },
                )
                .await
            {
                Ok(value) => value,
                Err(_) => {
                    Timer::after_secs(1).await;
                    continue;
                }
            };
            crate::health::mark(crate::health::Stage::BluetoothAdvertising);
            log::info!("ble: control service advertising");
            let connection = match advertiser.accept().await {
                Ok(value) => value,
                Err(_) => {
                    Timer::after_secs(1).await;
                    continue;
                }
            };
            let connection = match connection.with_attribute_server(&server) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(owner) = crate::ble_control::Session::new() else {
                return;
            };
            let mut receiver = ble_protocol::Receiver::new();
            let mut crypto: Option<ble_session::Session> = None;
            let mut response = ble_protocol::Response::new();
            let mut response_sequence = 0u16;
            let mut stream_sequence = 0u16;
            let mut stream = ble_protocol::stream::Sender::new();
            let mut stream_at = Instant::now();
            let mut probe = false;
            let mut authenticated = false;
            let mut acquired = false;
            let connected_at = Instant::now();
            let mut upload_at = connected_at;
            let mut plaintext = [0; ble_session::MAX_PLAINTEXT];
            crate::health::mark(crate::health::Stage::BluetoothConnected);
            log::info!("ble: connected");
            loop {
                // Check even when a peer supplies a continuous stream of GATT events.
                if !authenticated && connected_at.elapsed() > Duration::from_secs(30) {
                    break;
                }
                // Send at most one notification between GATT events. Credits bound the
                // controller queue; a stalled receiver cannot block Cancel or local Stop.
                if stream.enabled() && acquired {
                    if !stream.idle() && stream_at.elapsed() > Duration::from_secs(5) {
                        log::warn!("ble: stream ACK inactivity expiry");
                        break; // lost ACK: discard this incarnation, never replay its page
                    }
                    if stream.idle() {
                        let mut record = [0; ble_protocol::stream::PLAINTEXT_MAX];
                        let length = if probe {
                            probe = false;
                            let mut body = [0; ble_protocol::filesystem::PAGE_MAX];
                            for (i, b) in body.iter_mut().enumerate() {
                                *b = i as u8;
                            }
                            body[2040..2042]
                                .copy_from_slice(&connection.raw().att_mtu().to_le_bytes());
                            body[2042] = stream.high_water();
                            let high = crate::ble_relay::high_water();
                            body[2043] = high[0];
                            body[2044] = high[1];
                            body[2045..].fill(0);
                            ble_protocol::stream::encode(
                                ble_protocol::stream::PROBE,
                                &body,
                                &mut record,
                            )
                            .ok()
                        } else {
                            crate::ble_relay::take(&mut record)
                        };
                        if let Some(length) = length {
                            let Some(next) = stream_sequence.checked_add(1) else {
                                break;
                            };
                            stream_sequence = next;
                            let mut encrypted = [0; ble_protocol::stream::CIPHERTEXT_MAX];
                            let Some(session) = crypto.as_mut() else {
                                break;
                            };
                            let Ok(n) =
                                session.seal_stream(next, &record[..length], &mut encrypted)
                            else {
                                break;
                            };
                            ble_session::erase(&mut record);
                            if stream.set(next, &encrypted[..n]).is_err() {
                                break;
                            }
                            stream_at = Instant::now();
                        }
                    }
                    match stream.next_fragment() {
                        Ok(Some(frame)) => {
                            if !matches!(
                                embassy_time::with_timeout(
                                    Duration::from_millis(100),
                                    server.pico.events.notify(&connection, &frame)
                                )
                                .await,
                                Ok(Ok(()))
                            ) {
                                log::warn!("ble: notification send failed or exceeded 100ms");
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
                let event = match select(connection.next(), Timer::after_millis(2)).await {
                    Either::First(event) => event,
                    Either::Second(()) => {
                        if Instant::now().duration_since(upload_at) > Duration::from_secs(5) {
                            receiver.abandon();
                        }
                        // Fixed from accept: fragments/reads cannot extend unauthenticated ownership.
                        if !authenticated && connected_at.elapsed() > Duration::from_secs(30) {
                            break;
                        }
                        continue;
                    }
                };
                match event {
                    GattConnectionEvent::Disconnected { .. } => break,
                    GattConnectionEvent::Gatt { event } => {
                        stream_at = Instant::now(); // command uploads/finite reads may postpone stream pumping
                        let reply = match event {
                            GattEvent::Read(read) if read.handle() == server.pico.result.handle => {
                                match response.next_fragment() {
                                    Ok(frame) => {
                                        let _ = server.set(&server.pico.result, &frame);
                                        read.accept()
                                    }
                                    Err(_) => read.reject(AttErrorCode::UNLIKELY_ERROR),
                                }
                            }
                            GattEvent::Read(read) if read.handle() == server.pico.status.handle => {
                                let _ = server.set(&server.pico.status, &status());
                                read.accept()
                            }
                            GattEvent::Write(write)
                                if write.handle() == server.pico.auth.handle
                                    || write.handle() == server.pico.command.handle =>
                            {
                                let is_auth = write.handle() == server.pico.auth.handle;
                                if (is_auth && crypto.is_some()) || (!is_auth && crypto.is_none()) {
                                    break;
                                }
                                if upload_at.elapsed() > Duration::from_secs(5) {
                                    receiver.abandon();
                                }
                                upload_at = Instant::now();
                                let message = match receiver.push(write.data()) {
                                    Ok(value) => value,
                                    Err(_) => break,
                                };
                                if let Some(message) = message {
                                    if is_auth {
                                        if message.kind != ble_protocol::KIND_HANDSHAKE {
                                            break;
                                        }
                                        let Some(profile) = profile.as_ref() else {
                                            break;
                                        };
                                        let mut ephemeral = [0; 32];
                                        embassy_rp::clocks::RoscRng.fill_bytes(&mut ephemeral);
                                        let mut handshake =
                                            ble_session::Handshake::new(false, profile, ephemeral);
                                        ble_session::erase(&mut ephemeral);
                                        if handshake.read(message.payload).is_err() {
                                            break;
                                        }
                                        let mut answer = [0; ble_session::HANDSHAKE_LEN];
                                        if handshake.write(&mut answer).is_err() {
                                            break;
                                        }
                                        let Ok(session) = handshake.finish() else {
                                            break;
                                        };
                                        crypto = Some(session);
                                        if response
                                            .set(
                                                ble_protocol::KIND_HANDSHAKE,
                                                message.token,
                                                &answer,
                                            )
                                            .is_err()
                                        {
                                            break;
                                        }
                                    } else {
                                        if message.kind != ble_protocol::KIND_RECORD {
                                            break;
                                        }
                                        let Some(session) = crypto.as_mut() else {
                                            break;
                                        };
                                        let Ok(length) = session.open(
                                            message.token,
                                            message.payload,
                                            &mut plaintext,
                                        ) else {
                                            break;
                                        };
                                        let Some((&kind, body)) = plaintext[..length].split_first()
                                        else {
                                            break;
                                        };
                                        // No authority is derived from the unprotected fragment headers.
                                        authenticated = true;
                                        if kind == ble_protocol::KIND_FLOW && acquired {
                                            use ble_protocol::stream::Flow;
                                            let valid = match Flow::decode(body) {
                                                Ok(Flow::Start) => {
                                                    crate::ble_relay::activate(owner.0);
                                                    stream.start().is_ok()
                                                }
                                                Ok(Flow::Ack { sequence, offset }) => {
                                                    stream_at = Instant::now();
                                                    stream.ack(sequence, offset).is_ok()
                                                }
                                                Ok(Flow::Probe) if stream.idle() && !probe => {
                                                    probe = true;
                                                    true
                                                }
                                                _ => false,
                                            };
                                            ble_session::erase(&mut plaintext);
                                            if !valid {
                                                log::warn!("ble: invalid stream flow control");
                                                break;
                                            }
                                            // Flow grants have no finite response and never replace HID results.
                                            if let Ok(reply) = write.accept() {
                                                reply.send().await;
                                            }
                                            continue;
                                        }
                                        let mut filesystem_code = None;
                                        if kind == ble_protocol::KIND_FILESYSTEM
                                            && acquired
                                            && stream.enabled()
                                        {
                                            filesystem_code = Some(match body {
                                                [1, 0, request @ ..] => {
                                                    match ble_protocol::filesystem::Request::decode(
                                                        request,
                                                    ) {
                                                        Ok(request) => crate::ble_relay::list(
                                                            owner.0,
                                                            message.token,
                                                            request,
                                                        ),
                                                        Err(_) => Code::Rejected,
                                                    }
                                                }
                                                [1, 1, id @ ..] if id.len() == 8 => {
                                                    let mut value = [0; 8];
                                                    value.copy_from_slice(id);
                                                    crate::ble_relay::cancel(
                                                        owner.0,
                                                        u64::from_le_bytes(value),
                                                    );
                                                    // Discard an in-flight page. Stream nonces remain monotonic.
                                                    // Drain already credited bytes; the native receiver discards this ID.
                                                    Code::DirectoryCancelled
                                                }
                                                _ => Code::Rejected,
                                            });
                                        }
                                        let wants_overview = kind == ble_protocol::KIND_CONTROL
                                            && body == [ble_protocol::READ_OVERVIEW]
                                            && acquired;
                                        let wants_identity =
                                            kind == ble_protocol::KIND_IDENTITY && acquired;
                                        let mut identity_status =
                                            ble_protocol::identity::Status::Success;
                                        if filesystem_code.is_some() {
                                            // Separate finite admission result; preserve the HID result.
                                        } else if wants_identity {
                                            use ble_protocol::identity::{Command, Status};
                                            identity_status = match Command::decode(body) {
                                                Ok(Command::Read) => {
                                                    match crate::device_config::reload().await {
                                                        Ok(_) => Status::Success,
                                                        Err(_) => Status::FlashFailure,
                                                    }
                                                }
                                                Ok(Command::Save(identity)) => {
                                                    crate::device_config::save(identity)
                                                        .await
                                                        .err()
                                                        .unwrap_or(Status::Success)
                                                }
                                                Err(_) => Status::Invalid,
                                            };
                                        } else if kind == ble_protocol::KIND_CONTROL
                                            && (body == [ble_protocol::POLL] || wants_overview)
                                        {
                                            // Poll preserves the active Run's result token and completion.
                                        } else if matches!(
                                            kind,
                                            ble_protocol::KIND_CONTROL | ble_protocol::KIND_SCRIPT
                                        ) {
                                            if let Some(code) = command(
                                                owner.0,
                                                &mut acquired,
                                                message.token,
                                                kind,
                                                body,
                                            )
                                            .await
                                            {
                                                crate::ble_control::publish(message.token, code);
                                            }
                                        } else {
                                            break;
                                        }
                                        if !acquired {
                                            stream.reset();
                                            probe = false;
                                            crate::ble_relay::reset();
                                        }
                                        ble_session::erase(&mut plaintext);
                                        let Some(next) = response_sequence.checked_add(1) else {
                                            break;
                                        };
                                        response_sequence = next;
                                        let mut snapshot = [0; ble_protocol::MAX_OVERVIEW_LEN];
                                        let snapshot_len = if wants_overview {
                                            match overview(message.token, &mut snapshot) {
                                                Ok(length) => length,
                                                Err(_) => break,
                                            }
                                        } else if wants_identity {
                                            let config = crate::device_config::get().await;
                                            match (ble_protocol::identity::Response {
                                                request: message.token,
                                                status: identity_status,
                                                identity: config.identity(),
                                            })
                                            .encode(&mut snapshot)
                                            {
                                                Ok(length) => length,
                                                Err(_) => break,
                                            }
                                        } else {
                                            snapshot[..20].copy_from_slice(
                                                &filesystem_code.map_or_else(
                                                    crate::ble_control::result,
                                                    |code| {
                                                        ble_protocol::ResultValue {
                                                            token: message.token,
                                                            code,
                                                        }
                                                        .encode()
                                                    },
                                                ),
                                            );
                                            snapshot[20..ble_protocol::SNAPSHOT_LEN]
                                                .copy_from_slice(&status());
                                            ble_protocol::SNAPSHOT_LEN
                                        };
                                        let mut encrypted = [0; ble_protocol::MAX_RESPONSE_LEN];
                                        let Ok(length) = session.seal(
                                            next,
                                            &snapshot[..snapshot_len],
                                            &mut encrypted,
                                        ) else {
                                            break;
                                        };
                                        if response
                                            .set(
                                                ble_protocol::KIND_RECORD,
                                                next,
                                                &encrypted[..length],
                                            )
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                                write.accept()
                            }
                            GattEvent::Write(write)
                                if Some(write.handle()) == server.pico.events.cccd_handle =>
                            {
                                write.accept()
                            }
                            GattEvent::Write(write) => {
                                write.reject(AttErrorCode::WRITE_NOT_PERMITTED)
                            }
                            other => other.accept(),
                        };
                        if let Ok(reply) = reply {
                            reply.send().await;
                        }
                    }
                    _ => {}
                }
            }
            stream.reset();
            crate::ble_relay::reset();
            ble_session::erase(&mut plaintext);
            connection.raw().disconnect();
            drop(crypto);
            drop(owner); // Session drop cancels only this incarnation's remote effect.
            Timer::after_millis(250).await;
        }
    };
    select(runner.run(), service).await;
    log::error!("ble: service stopped after HCI failure");
}

async fn command(
    session: u32,
    acquired: &mut bool,
    token: u16,
    kind: u8,
    message: &[u8],
) -> Option<Code> {
    if kind == ble_protocol::KIND_CONTROL {
        return Some(match message {
            [ble_protocol::ACQUIRE] => {
                *acquired = true;
                Code::Acquired
            }
            [ble_protocol::RELEASE] if *acquired && crate::usb::hid::active_job().is_none() => {
                *acquired = false;
                Code::Released
            }
            [ble_protocol::USB_ON] if *acquired => {
                if crate::usb::usb_supervisor::start().await.is_ok() {
                    Code::UsbChanged
                } else {
                    Code::Busy
                }
            }
            [ble_protocol::USB_OFF] if *acquired && crate::usb::hid::active_job().is_none() => {
                let stopped = embassy_time::with_timeout(
                    Duration::from_secs(2),
                    crate::usb::usb_supervisor::stop(150),
                )
                .await;
                if matches!(stopped, Ok(Ok(()))) {
                    Code::UsbChanged
                } else {
                    Code::Busy
                }
            }
            _ => Code::Rejected,
        });
    }
    if !*acquired {
        return Some(Code::Rejected);
    }
    match script_protocol::decode(message) {
        Ok(script_protocol::Message::Run { id, bytecode }) => {
            let mut program = heapless::Vec::new();
            if program.extend_from_slice(bytecode).is_err() {
                return Some(Code::Rejected);
            }
            match crate::usb::hid::submit_companion(session, id, program) {
                Ok(_) => {
                    crate::ble_control::admitted(id, token);
                    None
                }
                Err(crate::usb::hid::SubmitError::Busy) => Some(Code::Busy),
                Err(crate::usb::hid::SubmitError::UsbUnavailable) => Some(Code::UsbUnavailable),
                Err(crate::usb::hid::SubmitError::Invalid) => Some(Code::Rejected),
            }
        }
        Ok(script_protocol::Message::Cancel { id }) => {
            if crate::ble_control::active(id) {
                crate::usb::hid::cancel_companion(session, id.process_id, id.effect_id);
                None
            } else {
                Some(Code::Cancelled)
            }
        }
        _ => Some(Code::Rejected),
    }
}

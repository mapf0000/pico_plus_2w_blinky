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

const SERVICE_UUID: Uuid = Uuid::new_long(ble_protocol::SERVICE_UUID.to_le_bytes());
const INFO_UUID: Uuid = Uuid::new_long(ble_protocol::INFO_UUID.to_le_bytes());
const STATUS_UUID: Uuid = Uuid::new_long(ble_protocol::STATUS_UUID.to_le_bytes());
const AUTH_UUID: Uuid = Uuid::new_long(ble_protocol::AUTH_UUID.to_le_bytes());
const COMMAND_UUID: Uuid = Uuid::new_long(ble_protocol::COMMAND_UUID.to_le_bytes());
const RESULT_UUID: Uuid = Uuid::new_long(ble_protocol::RESULT_UUID.to_le_bytes());
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
                let event = match select(connection.next(), Timer::after_millis(100)).await {
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
                                        if kind == ble_protocol::KIND_CONTROL
                                            && body == [ble_protocol::POLL]
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
                                        ble_session::erase(&mut plaintext);
                                        let Some(next) = response_sequence.checked_add(1) else {
                                            break;
                                        };
                                        response_sequence = next;
                                        let mut snapshot = [0; ble_protocol::SNAPSHOT_LEN];
                                        snapshot[..20]
                                            .copy_from_slice(&crate::ble_control::result());
                                        snapshot[20..].copy_from_slice(&status());
                                        let mut encrypted = [0; ble_protocol::MAX_RESPONSE_LEN];
                                        let Ok(length) =
                                            session.seal(next, &snapshot, &mut encrypted)
                                        else {
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
                let _ = crate::usb::usb_supervisor::start().await;
                Code::UsbChanged
            }
            [ble_protocol::USB_OFF] if *acquired && crate::usb::hid::active_job().is_none() => {
                let _ = embassy_time::with_timeout(
                    Duration::from_secs(2),
                    crate::usb::usb_supervisor::stop(150),
                )
                .await;
                Code::UsbChanged
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

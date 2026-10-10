//! Bluetooth-only control service with authenticated numeric-comparison pairing.
use ble_protocol::Code;
use embassy_futures::select::{Either3, select, select3};
use embassy_time::{Duration, Instant, Timer};
// TrouBLE 0.6 macros name embassy_sync directly. Scope its 0.7 API here;
// firmware services continue using 0.8 without changing their synchronization.
use embassy_sync_ble as embassy_sync;
use portable_atomic::Ordering;
use static_cell::StaticCell;
use trouble_host::prelude::*;

const SERVICE_UUID: Uuid = Uuid::new_long(ble_protocol::SERVICE_UUID.to_le_bytes());
const INFO_UUID: Uuid = Uuid::new_long(ble_protocol::INFO_UUID.to_le_bytes());
const STATUS_UUID: Uuid = Uuid::new_long(ble_protocol::STATUS_UUID.to_le_bytes());
const PAIR_UUID: Uuid = Uuid::new_long(ble_protocol::PAIR_UUID.to_le_bytes());
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
    #[characteristic(uuid = PAIR_UUID, read)]
    pair: u8,
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
        uptime_secs: embassy_time::Instant::now().as_secs(),
    }
    .encode()
}

#[embassy_executor::task]
pub async fn task(driver: cyw43::bluetooth::BtDriver<'static>, address: [u8; 6]) {
    // One peripheral, signalling + ATT + SMP. Eight 128-byte packets, no heap.
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 3>> = StaticCell::new();
    let resources = RESOURCES.init(HostResources::new());
    let controller = ExternalController::<_, 10>::new(driver);
    let stack =
        trouble_host::new(controller, resources).set_random_address(Address::random(address));
    let stack = stack.set_random_generator_seed(&mut embassy_rp::clocks::RoscRng);
    stack.set_io_capabilities(IoCapabilities::DisplayYesNo);
    let Host {
        mut peripheral,
        mut runner,
        ..
    } = stack.build();
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
        log::error!("ble: info initialization failed");
        return;
    }
    if server.set(&server.pico.pair, &1).is_err() {
        return;
    }
    let uuids = [ble_protocol::SERVICE_UUID.to_le_bytes()];
    let mut ad = [0; 31];
    let ad_len = match AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ServiceUuids128(&uuids),
        ],
        &mut ad,
    ) {
        Ok(len) => len,
        Err(_) => {
            log::error!("ble: advertising encoding failed");
            return;
        }
    };
    let mut scan = [0; 31];
    let scan_len = match AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(NAME.as_bytes())],
        &mut scan,
    ) {
        Ok(len) => len,
        Err(_) => {
            log::error!("ble: name encoding failed");
            return;
        }
    };
    let service = async {
        loop {
            let advertisement = Advertisement::ConnectableScannableUndirected {
                adv_data: &ad[..ad_len],
                scan_data: &scan[..scan_len],
            };
            let advertiser = match peripheral
                .advertise(&Default::default(), advertisement)
                .await
            {
                Ok(advertiser) => advertiser,
                Err(_) => {
                    log::warn!("ble: advertising failed; retrying");
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                }
            };
            crate::health::mark(crate::health::Stage::BluetoothAdvertising);
            log::info!("ble: control service advertising");
            let connection = match advertiser.accept().await {
                Ok(connection) => connection,
                Err(_) => {
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                }
            };
            let connection = match connection.with_attribute_server(&server) {
                Ok(connection) => connection,
                Err(_) => {
                    log::warn!("ble: ATT connection rejected");
                    continue;
                }
            };
            let Some(session) = crate::ble_control::Session::new() else {
                return;
            };
            if connection.raw().set_bondable(false).is_err() {
                continue;
            }
            let mut receiver = ble_protocol::Receiver::new();
            let mut upload_at = Instant::now();
            let mut pairing_at = Instant::now();
            let mut confirmed = false;
            let mut acquired = false;
            crate::health::mark(crate::health::Stage::BluetoothConnected);
            log::info!("ble: connected");
            loop {
                let event = match select3(
                    connection.next(),
                    crate::ble_control::DECISION.wait(),
                    Timer::after_millis(100),
                )
                .await
                {
                    Either3::First(event) => event,
                    Either3::Second(yes) => {
                        if crate::ble_control::pairing_code().is_some() {
                            confirmed = yes;
                            if yes {
                                let _ = connection.pass_key_confirm();
                            } else {
                                let _ = connection.pass_key_cancel();
                            }
                            crate::ble_control::clear_pairing();
                        }
                        continue;
                    }
                    Either3::Third(()) => {
                        if Instant::now().duration_since(upload_at) > Duration::from_secs(5) {
                            receiver.abandon();
                        }
                        if crate::ble_control::pairing_code().is_some()
                            && Instant::now().duration_since(pairing_at) > Duration::from_secs(30)
                        {
                            let _ = connection.pass_key_cancel();
                            crate::ble_control::clear_pairing();
                        }
                        continue;
                    }
                };
                match event {
                    GattConnectionEvent::Disconnected { .. } => {
                        log::info!("ble: disconnected");
                        break;
                    }
                    GattConnectionEvent::PassKeyConfirm(key) => {
                        confirmed = false;
                        pairing_at = Instant::now();
                        crate::ble_control::begin_pairing(key.value());
                    }
                    // Only numeric comparison with physical confirmation authorizes control.
                    GattConnectionEvent::PassKeyInput | GattConnectionEvent::PassKeyDisplay(_) => {
                        let _ = connection.pass_key_cancel();
                    }
                    GattConnectionEvent::PairingFailed(_) => {
                        confirmed = false;
                        acquired = false;
                        crate::ble_control::clear_pairing();
                    }
                    GattConnectionEvent::Gatt { event } => {
                        let authenticated = confirmed
                            && connection
                                .raw()
                                .security_level()
                                .is_ok_and(|level| level.authenticated());
                        let _ = server.set(&server.pico.status, &status());
                        let _ = server.set(&server.pico.result, &crate::ble_control::result());
                        let reply = match event {
                            GattEvent::Read(read)
                                if read.handle() == server.pico.pair.handle && !authenticated =>
                            {
                                let _ = connection.raw().request_security();
                                read.reject(AttErrorCode::INSUFFICIENT_AUTHENTICATION)
                            }
                            GattEvent::Write(write)
                                if write.handle() == server.pico.command.handle =>
                            {
                                if !authenticated {
                                    write.reject(AttErrorCode::INSUFFICIENT_AUTHENTICATION)
                                } else {
                                    upload_at = Instant::now();
                                    match receiver.push(write.data()) {
                                        Ok(Some(ble_protocol::Message {
                                            token,
                                            kind,
                                            payload: message,
                                        })) => {
                                            let code = command(
                                                session.0,
                                                &mut acquired,
                                                token,
                                                kind,
                                                message,
                                            )
                                            .await;
                                            if let Some(code) = code {
                                                crate::ble_control::publish(token, code);
                                            }
                                            // Attribute storage is bounded to twenty bytes; parsing uses the exact write length.
                                            write.accept()
                                        }
                                        Ok(None) => write.accept(),
                                        Err(_) => write
                                            .reject(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH),
                                    }
                                }
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
        }
    };
    // USB and the physical display remain alive on HCI failure.
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

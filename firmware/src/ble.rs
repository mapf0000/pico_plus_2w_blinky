//! Opt-in, read-only BLE feasibility service. No HID/host command admission.
use embassy_futures::select::select;
// TrouBLE 0.6 macros name embassy_sync directly. Scope its 0.7 API here;
// firmware services continue using 0.8 without changing their synchronization.
use embassy_sync_ble as embassy_sync;
use portable_atomic::Ordering;
use static_cell::StaticCell;
use trouble_host::prelude::*;

const SERVICE_UUID: Uuid = Uuid::new_long(ble_protocol::SERVICE_UUID.to_le_bytes());
const INFO_UUID: Uuid = Uuid::new_long(ble_protocol::INFO_UUID.to_le_bytes());
const STATUS_UUID: Uuid = Uuid::new_long(ble_protocol::STATUS_UUID.to_le_bytes());
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
    // One peripheral connection and only its signalling + ATT channels. The
    // shared packet pool has eight 27-byte packets; no bulk payloads or heap.
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 2>> = StaticCell::new();
    let resources = RESOURCES.init(HostResources::new());
    let controller = ExternalController::<_, 10>::new(driver);
    let stack =
        trouble_host::new(controller, resources).set_random_address(Address::random(address));
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
            log::info!("ble: read-only service advertising");
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
            log::info!("ble: connected (read-only)");
            loop {
                match connection.next().await {
                    GattConnectionEvent::Disconnected { .. } => {
                        log::info!("ble: disconnected");
                        break;
                    }
                    GattConnectionEvent::Gatt { event } => {
                        // Refresh before *any* ATT read form (including read-by-type).
                        // Characteristic metadata permits reads only; the server
                        // also rejects writes even when no application handler runs.
                        if server.set(&server.pico.status, &status()).is_err() {
                            log::warn!("ble: status update failed");
                        }
                        let reply = match event {
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
    // A fatal HCI error stops only BLE. WLAN and USB tasks keep running.
    select(runner.run(), service).await;
    log::error!("ble: service stopped after HCI failure");
}

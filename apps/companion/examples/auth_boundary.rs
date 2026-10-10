//! Explicit hardware acceptance: no credentials, valid encrypted commands or HID.
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use std::{
    error::Error,
    time::{Duration, Instant},
};
use uuid::Uuid;

const SERVICE: Uuid = Uuid::from_u128(ble_protocol::SERVICE_UUID);
type TestResult<T> = Result<T, Box<dyn Error>>;

async fn connect(adapter: &Adapter) -> TestResult<Peripheral> {
    adapter
        .start_scan(ScanFilter {
            services: vec![SERVICE],
        })
        .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    adapter.stop_scan().await?;
    let mut found = Vec::new();
    for peripheral in adapter.peripherals().await?.into_iter().take(256) {
        if peripheral
            .properties()
            .await?
            .is_some_and(|p| p.services.contains(&SERVICE))
        {
            found.push(peripheral);
        }
    }
    if found.len() != 1 {
        return Err(
            "Expected exactly one Pico service; disconnect other controllers and retry".into(),
        );
    }
    let peripheral = found.remove(0);
    peripheral.connect().await?;
    peripheral.discover_services().await?;
    let info = characteristic(&peripheral, ble_protocol::INFO_UUID)?;
    ble_protocol::Info::decode(&peripheral.read(&info).await?).map_err(|_| "Expected BLE v3")?;
    Ok(peripheral)
}
fn characteristic(
    peripheral: &Peripheral,
    uuid: u128,
) -> TestResult<btleplug::api::Characteristic> {
    peripheral
        .characteristics()
        .into_iter()
        .find(|c| c.service_uuid == SERVICE && c.uuid == Uuid::from_u128(uuid))
        .ok_or_else(|| "Missing characteristic".into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> TestResult<()> {
    tokio::time::timeout(Duration::from_secs(50), check())
        .await
        .map_err(|_| "Boundary test timed out")?
}
async fn check() -> TestResult<()> {
    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or("No Bluetooth adapter")?;
    let peripheral = connect(&adapter).await?;
    let command = characteristic(&peripheral, ble_protocol::COMMAND_UUID)?;
    let status = characteristic(&peripheral, ble_protocol::STATUS_UUID)?;
    // Deliberately unauthenticated and unencrypted. No valid record is constructed.
    let (frame, length) = ble_protocol::fragment(
        ble_protocol::KIND_RECORD,
        1,
        &[ble_protocol::KIND_CONTROL, ble_protocol::ACQUIRE],
        0,
    )
    .map_err(|_| "Invalid test frame")?;
    let _ = peripheral
        .write(&command, &frame[..length], WriteType::WithResponse)
        .await;
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !peripheral.is_connected().await.unwrap_or(false)
                || peripheral.read(&status).await.is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_ok();
    let _ = peripheral.disconnect().await;
    if !closed {
        return Err("Unauthenticated command did not close the connection".into());
    }
    println!("Unauthenticated plaintext Acquire rejected; no control or HID access.");
    let peripheral = connect(&adapter).await?;
    let status = characteristic(&peripheral, ble_protocol::STATUS_UUID)?;
    let started = Instant::now();
    let mut reads = 0;
    loop {
        match tokio::time::timeout(Duration::from_secs(2), peripheral.read(&status)).await {
            Ok(Ok(bytes)) => {
                ble_protocol::Status::decode(&bytes).map_err(|_| "Invalid public status")?;
                reads += 1;
                if started.elapsed() > Duration::from_secs(35) {
                    let _ = peripheral.disconnect().await;
                    return Err("Public traffic extended the authentication deadline".into());
                }
            }
            _ => break,
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let elapsed = started.elapsed();
    let _ = peripheral.disconnect().await;
    if elapsed < Duration::from_secs(28) || elapsed > Duration::from_secs(35) {
        return Err("Connection failed outside the expected authentication timeout".into());
    }
    println!(
        "Unauthenticated connection expired after {:.2}s despite {reads} continuous status reads.",
        elapsed.as_secs_f64()
    );
    Ok(())
}

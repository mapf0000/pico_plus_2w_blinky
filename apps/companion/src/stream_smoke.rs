//! Read-only stream/filesystem hardware acceptance. Never emits HID or saves flash.
use crate::ble::BleTransport;
use companion_core::{Event, Failure, Operation, Reply, Request, Tag, Transport};
use std::time::{Duration, Instant};
pub fn run(
    profile: ble_session::provisioning::Profile,
    path: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut transport = BleTransport::new(Some(profile));
            let mut request = 0;
            let result = check(&mut transport, &mut request, path).await;
            transport.shutdown().await;
            result.map_err(|e| e.message().into())
        })
}
async fn call(
    t: &mut BleTransport,
    next: &mut u64,
    epoch: u64,
    operation: Operation,
) -> Result<Reply, Failure> {
    println!(
        "Stream check operation: {}",
        match &operation {
            Operation::Scan => "scan",
            Operation::Connect(_) => "connect",
            Operation::Acquire => "acquire",
            Operation::Release => "release",
            Operation::StreamProbe => "probe",
            Operation::ListDirectory { .. } => "directory",
            _ => "control",
        }
    );
    *next += 1;
    let tag = Tag {
        epoch,
        request: *next,
    };
    t.submit(Request { tag, operation }, Duration::ZERO)?;
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        while let Some(event) = t.poll(Duration::ZERO) {
            match event {
                Event::Reply { tag: other, reply } if other == tag => {
                    return match reply {
                        Reply::Error(error) => Err(error),
                        other => Ok(other),
                    };
                }
                Event::Lost { epoch: other } if other == epoch => return Err(Failure::LinkLost),
                _ => {}
            }
        }
        if Instant::now() > deadline {
            return Err(Failure::Timeout);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn check(t: &mut BleTransport, next: &mut u64, path: Option<String>) -> Result<(), Failure> {
    let Reply::Devices(devices) = call(t, next, 1, Operation::Scan).await? else {
        return Err(Failure::InvalidData);
    };
    let device = devices.first().ok_or(Failure::NotReady)?.clone();
    let Reply::Connected(caps) = call(t, next, 1, Operation::Connect(device.clone())).await? else {
        return Err(Failure::InvalidData);
    };
    if !caps.filesystem_supported {
        return Err(Failure::Incompatible);
    }
    if !matches!(call(t, next, 1, Operation::Acquire).await?, Reply::Acquired) {
        return Err(Failure::NotReady);
    }
    for _ in 0..3 {
        let Reply::StreamMeasured {
            bytes,
            elapsed,
            att_mtu,
            queue_high_water,
        } = call(t, next, 1, Operation::StreamProbe).await?
        else {
            return Err(Failure::InvalidData);
        };
        println!(
            "Encrypted stream: {bytes} bytes in {:.3}s ({:.0} bytes/s), ATT MTU {att_mtu}, values 20 bytes, window 4, queue high-water {queue_high_water:?}",
            elapsed.as_secs_f64(),
            bytes as f64 / elapsed.as_secs_f64()
        );
    }
    if let Some(path) = path {
        let mut cursor = 0;
        let mut directory = path;
        let mut total = 0;
        loop {
            let id = *next + 1;
            let Reply::Directory(bytes) = call(
                t,
                next,
                1,
                Operation::ListDirectory {
                    path: directory.clone(),
                    cursor,
                    hidden: true,
                },
            )
            .await?
            else {
                return Err(Failure::InvalidData);
            };
            let page =
                ble_protocol::filesystem::Page::decode(&bytes).map_err(|_| Failure::InvalidData)?;
            if page.id != id || page.status != 0 || page.next_cursor != cursor + page.count as u32 {
                return Err(Failure::InvalidData);
            }
            total += page.count;
            println!(
                "Filesystem page: {} bytes, {} entries, cursor {}, more {}",
                bytes.len(),
                page.count,
                page.next_cursor,
                page.more
            );
            directory = page.directory.into();
            cursor = page.next_cursor;
            if !page.more {
                break;
            }
            if total >= 512 {
                return Err(Failure::InvalidData);
            }
        }
        let id = *next + 1;
        let Reply::Directory(bytes) = call(
            t,
            next,
            1,
            Operation::ListDirectory {
                path: format!("{directory}/pico-companion-missing-path-20261011"),
                cursor: 0,
                hidden: false,
            },
        )
        .await?
        else {
            return Err(Failure::InvalidData);
        };
        let page =
            ble_protocol::filesystem::Page::decode(&bytes).map_err(|_| Failure::InvalidData)?;
        if page.id != id || page.status != 1 {
            return Err(Failure::InvalidData);
        }
        println!("Filesystem missing-path error passed; {total} total entries.");
        // Exercise both an unstarted request and an in-flight stream. A subsequent
        // list must discard the cancelled page and retain this authenticated session.
        for delay in [Duration::ZERO, Duration::from_secs(1)] {
            *next += 1;
            let tag = Tag {
                epoch: 1,
                request: *next,
            };
            t.submit(
                Request {
                    tag,
                    operation: Operation::ListDirectory {
                        path: directory.clone(),
                        cursor: 0,
                        hidden: false,
                    },
                },
                Duration::ZERO,
            )?;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let started = Instant::now();
            t.submit(
                Request {
                    tag,
                    operation: Operation::CancelDirectory,
                },
                Duration::ZERO,
            )?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(event) = t.poll(Duration::ZERO) {
                    match event {
                        Event::Reply {
                            tag: other,
                            reply: Reply::DirectoryCancelled,
                        } if other == tag => break,
                        Event::Reply {
                            tag: other,
                            reply: Reply::Error(error),
                        } if other == tag => return Err(error),
                        Event::Lost { epoch: 1 } => return Err(Failure::LinkLost),
                        _ => {}
                    }
                }
                if Instant::now() > deadline {
                    return Err(Failure::Timeout);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            println!(
                "Directory cancellation (request age {:.1}s): {:.3}s",
                delay.as_secs_f64(),
                started.elapsed().as_secs_f64()
            );
            let id = *next + 1;
            let Reply::Directory(bytes) = call(
                t,
                next,
                1,
                Operation::ListDirectory {
                    path: directory.clone(),
                    cursor: 0,
                    hidden: false,
                },
            )
            .await?
            else {
                return Err(Failure::InvalidData);
            };
            let page =
                ble_protocol::filesystem::Page::decode(&bytes).map_err(|_| Failure::InvalidData)?;
            if page.id != id || page.status != 0 {
                return Err(Failure::InvalidData);
            }
            println!("Fresh listing after cancellation passed.");
        }
        let Reply::StreamMeasured {
            queue_high_water, ..
        } = call(t, next, 1, Operation::StreamProbe).await?
        else {
            return Err(Failure::InvalidData);
        };
        if queue_high_water[2] != 1 {
            return Err(Failure::InvalidData);
        }
        println!(
            "Post-browsing queue high-water {queue_high_water:?} (fragments, buttons, pages)."
        );
    }
    if !matches!(call(t, next, 1, Operation::Release).await?, Reply::Released) {
        return Err(Failure::InvalidData);
    }
    t.close();
    tokio::time::sleep(Duration::from_millis(800)).await;
    call(t, next, 2, Operation::Connect(device)).await?;
    call(t, next, 2, Operation::Acquire).await?;
    if !matches!(
        call(t, next, 2, Operation::StreamProbe).await?,
        Reply::StreamMeasured { bytes: 2048, .. }
    ) {
        return Err(Failure::InvalidData);
    }
    call(t, next, 2, Operation::Release).await?;
    println!(
        "Stream acceptance passed: subscription, encrypted maximum records, credits, fresh reconnect. No USB identity or HID changes."
    );
    Ok(())
}

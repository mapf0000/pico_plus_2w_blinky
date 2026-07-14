use crate::config::Config;
use crate::tlv::Frame;
use crate::transport::{self, Event};
use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep, timeout_at};

const TAG_DEBUG_MSG: u8 = 2;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_AGENT_STATUS: u8 = 8;
const HANDSHAKE: &[u8] = b"handshake";
const HANDSHAKE_OK: &[u8] = b"handshake-ok";
const PROBE_OK: &[u8] = b"probe-ok";
const SELF_TEST_IDENTITY: &str = "device-self-test";

#[derive(Clone, Copy)]
struct Options {
    keepalives: u8,
    interval: Duration,
    response_timeout: Duration,
}

pub(crate) async fn run(config: &Config) -> Result<()> {
    ensure!(
        config.send_files.is_empty(),
        "--send-file is forbidden in device self-test mode"
    );
    ensure!(
        !config.raw,
        "--raw is forbidden in device self-test mode because it may log payload bytes"
    );
    ensure!(
        config.port.is_some() || (config.vid.is_some() && config.pid.is_some()),
        "device self-test requires --port or both --vid and --pid"
    );

    let port = transport::select_port_uncached(config).await?;
    println!("[PASS] Host agent selected the control CDC without using its cache");
    let (inbound, outbound) = transport::spawn(port, false).await?;
    let options = Options {
        keepalives: config.self_test_keepalives,
        interval: Duration::from_millis(config.self_test_interval_ms),
        response_timeout: Duration::from_millis(config.self_test_timeout_ms),
    };
    run_protocol(inbound, outbound, options).await?;
    println!(
        "[PASS] Host-agent restricted device self-test complete ({} keepalives)",
        options.keepalives
    );
    Ok(())
}

async fn run_protocol(
    mut inbound: mpsc::Receiver<Event>,
    outbound: mpsc::Sender<Frame>,
    options: Options,
) -> Result<()> {
    send(&outbound, TAG_REQUEST_AGENT_STATUS, HANDSHAKE).await?;
    send(
        &outbound,
        TAG_AGENT_STATUS,
        &crate::agent_status::payload_for(SELF_TEST_IDENTITY),
    )
    .await?;
    await_debug(
        &mut inbound,
        &outbound,
        HANDSHAKE_OK,
        options.response_timeout,
    )
    .await?;
    println!("[PASS] Host-agent handshake completed and fixed test status was sent");

    for index in 0..options.keepalives {
        sleep(options.interval).await;
        send(&outbound, TAG_REQUEST_AGENT_STATUS, &[]).await?;
        await_debug(&mut inbound, &outbound, PROBE_OK, options.response_timeout).await?;
        println!(
            "[PASS] Host-agent keepalive round-trip {}/{}",
            index + 1,
            options.keepalives
        );
    }

    Ok(())
}

async fn send(outbound: &mpsc::Sender<Frame>, tag: u8, payload: &[u8]) -> Result<()> {
    outbound
        .send(Frame::new(tag, Bytes::copy_from_slice(payload)))
        .await
        .with_context(|| format!("queue self-test tag {tag}"))
}

async fn await_debug(
    inbound: &mut mpsc::Receiver<Event>,
    outbound: &mpsc::Sender<Frame>,
    expected: &[u8],
    response_timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + response_timeout;
    loop {
        let event = timeout_at(deadline, inbound.recv())
            .await
            .context("device self-test response timed out")?
            .context("serial transport closed during device self-test")?;
        let Event::Frame(frame) = event else {
            bail!("restricted device self-test received raw serial data");
        };

        match frame.tag {
            TAG_DEBUG_MSG => {
                ensure!(
                    frame.payload.as_ref() == expected,
                    "unexpected device debug response ({} bytes)",
                    frame.payload.len()
                );
                return Ok(());
            }
            TAG_REQUEST_AGENT_STATUS => {
                send(
                    outbound,
                    TAG_AGENT_STATUS,
                    &crate::agent_status::payload_for(SELF_TEST_IDENTITY),
                )
                .await?;
            }
            tag => {
                bail!(
                    "restricted device self-test rejected unexpected device tag {tag} ({} bytes)",
                    frame.payload.len()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels() -> (
        mpsc::Sender<Event>,
        mpsc::Receiver<Event>,
        mpsc::Sender<Frame>,
        mpsc::Receiver<Frame>,
    ) {
        let (device_to_agent_tx, device_to_agent_rx) = mpsc::channel(8);
        let (agent_to_device_tx, agent_to_device_rx) = mpsc::channel(8);
        (
            device_to_agent_tx,
            device_to_agent_rx,
            agent_to_device_tx,
            agent_to_device_rx,
        )
    }

    async fn next_frame(receiver: &mut mpsc::Receiver<Frame>) -> Frame {
        tokio::time::timeout(Duration::from_millis(100), receiver.recv())
            .await
            .expect("frame timeout")
            .expect("frame channel closed")
    }

    #[tokio::test]
    async fn restricted_protocol_runs_handshake_status_and_keepalives() {
        let (device_tx, agent_rx, agent_tx, mut device_rx) = channels();
        let options = Options {
            keepalives: 2,
            interval: Duration::from_millis(1),
            response_timeout: Duration::from_millis(100),
        };
        let agent = tokio::spawn(run_protocol(agent_rx, agent_tx, options));

        let handshake = next_frame(&mut device_rx).await;
        assert_eq!(handshake.tag, TAG_REQUEST_AGENT_STATUS);
        assert_eq!(handshake.payload, HANDSHAKE);
        let status = next_frame(&mut device_rx).await;
        assert_eq!(status.tag, TAG_AGENT_STATUS);
        assert!(status.payload.ends_with(SELF_TEST_IDENTITY.as_bytes()));
        device_tx
            .send(Event::Frame(Frame::new(
                TAG_DEBUG_MSG,
                Bytes::from_static(HANDSHAKE_OK),
            )))
            .await
            .unwrap();

        for _ in 0..options.keepalives {
            let probe = next_frame(&mut device_rx).await;
            assert_eq!(probe.tag, TAG_REQUEST_AGENT_STATUS);
            assert!(probe.payload.is_empty());
            device_tx
                .send(Event::Frame(Frame::new(
                    TAG_DEBUG_MSG,
                    Bytes::from_static(PROBE_OK),
                )))
                .await
                .unwrap();
        }

        agent.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn restricted_protocol_rejects_commands_without_dispatching() {
        let (device_tx, agent_rx, agent_tx, mut device_rx) = channels();
        let options = Options {
            keepalives: 1,
            interval: Duration::from_millis(1),
            response_timeout: Duration::from_millis(100),
        };
        let agent = tokio::spawn(run_protocol(agent_rx, agent_tx, options));

        let _ = next_frame(&mut device_rx).await;
        let _ = next_frame(&mut device_rx).await;
        device_tx
            .send(Event::Frame(Frame::new(1, Bytes::from_static(b"ignored"))))
            .await
            .unwrap();

        let error = agent.await.unwrap().unwrap_err().to_string();
        assert!(error.contains("rejected unexpected device tag 1"));
    }
}

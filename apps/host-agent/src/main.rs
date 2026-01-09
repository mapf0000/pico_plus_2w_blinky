mod config;
mod dispatch;
mod tlv;
mod transport;

use anyhow::Result;
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::Config::from_env()?;
    config::init_logging(config.debug)?;

    if let Some(cwd) = &config.cwd {
        std::env::set_current_dir(cwd)?;
    }

    run_daemon(config).await?;

    Ok(())
}

async fn run_daemon(config: config::Config) -> Result<()> {
    #[cfg(any(test, feature = "test-port-fd"))]
    if let Some(fd) = config.port_fd {
        #[cfg(unix)]
        {
            let (inbound, outbound) = transport::spawn_fd(fd, config.raw).await?;
            if let Err(err) = dispatch::run(inbound, outbound, &config).await {
                warn!(error = %err, "dispatch loop failed");
            }
            return Ok(());
        }
        #[cfg(not(unix))]
        {
            anyhow::bail!("--port-fd is only supported on unix targets");
        }
    }

    let mut backoff = Duration::from_millis(250);
    let max_backoff = Duration::from_secs(5);
    let mut attempt: u64 = 0;

    debug!(
        vid = ?config.vid,
        pid = ?config.pid,
        port = ?config.port,
        probe_timeout_ms = config.probe_timeout_ms,
        raw = config.raw,
        "serial configuration"
    );

    loop {
        attempt = attempt.saturating_add(1);
        info!(
            attempt,
            backoff_ms = backoff.as_millis(),
            "starting connection attempt"
        );
        match transport::select_port(&config).await {
            Ok(port) => {
                info!(attempt, port = %port, "connecting to device");
                match transport::spawn(port, config.raw).await {
                    Ok((inbound, outbound)) => {
                        backoff = Duration::from_millis(250);
                        if let Err(err) = dispatch::run(inbound, outbound, &config).await {
                            warn!(attempt, error = %err, "dispatch loop failed; reconnecting");
                        } else {
                            warn!(attempt, "connection closed; reconnecting");
                        }
                        debug!(attempt, "clearing cached port after disconnect");
                        transport::clear_cached_port();
                    }
                    Err(err) => {
                        warn!(attempt, error = %err, "failed to open serial port");
                    }
                }
            }
            Err(err) => {
                warn!(attempt, error = %err, "failed to select port");
            }
        }

        info!(
            attempt,
            backoff_ms = backoff.as_millis(),
            "waiting before reconnect"
        );
        sleep(backoff).await;
        let next_backoff = std::cmp::min(backoff * 2, max_backoff);
        debug!(
            attempt,
            backoff_ms = next_backoff.as_millis(),
            "updated reconnect backoff"
        );
        backoff = next_backoff;
    }
}

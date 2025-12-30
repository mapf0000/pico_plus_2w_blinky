mod config;
mod dispatch;
mod tlv;
mod transport;

use anyhow::Result;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

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
    let mut backoff = Duration::from_millis(250);
    let max_backoff = Duration::from_secs(5);

    loop {
        match transport::select_port(&config).await {
            Ok(port) => {
                info!(port = %port, "connecting to device");
                match transport::spawn(port, config.raw).await {
                    Ok((inbound, outbound)) => {
                        backoff = Duration::from_millis(250);
                        if let Err(err) = dispatch::run(inbound, outbound, &config).await {
                            warn!(error = %err, "dispatch loop failed");
                        } else {
                            warn!("connection closed; reconnecting");
                        }
                    }
                    Err(err) => {
                        warn!(error = %err, "failed to open serial port");
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to select port");
            }
        }

        sleep(backoff).await;
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
}

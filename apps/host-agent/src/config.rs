use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub port: Option<String>,
    #[cfg(any(test, feature = "test-port-fd"))]
    pub port_fd: Option<i32>,
    pub cwd: Option<PathBuf>,
    pub probe_timeout_ms: u64,
    pub debug_log: Option<PathBuf>,
    pub send_files: Vec<PathBuf>,
    pub raw: bool,
    pub debug: bool,
    pub device_self_test: bool,
    pub self_test_keepalives: u8,
    pub self_test_interval_ms: u64,
    pub self_test_timeout_ms: u64,
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(long)]
    vid: Option<String>,
    #[arg(long)]
    pid: Option<String>,
    #[arg(long)]
    port: Option<String>,
    #[cfg(any(test, feature = "test-port-fd"))]
    #[arg(long)]
    port_fd: Option<i32>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long, default_value_t = 400)]
    probe_timeout_ms: u64,
    #[arg(long)]
    debug_log: Option<PathBuf>,
    #[arg(
        long = "send-file",
        help = "Set a default file candidate; a browser session must still request the transfer"
    )]
    send_file: Vec<PathBuf>,
    #[arg(long)]
    raw: bool,
    #[arg(long)]
    debug: bool,
    #[arg(
        long,
        help = "Run a bounded handshake/keepalive device test without the general dispatcher"
    )]
    device_self_test: bool,
    #[arg(long, default_value_t = 2)]
    self_test_keepalives: u8,
    #[arg(long, default_value_t = 10_000)]
    self_test_interval_ms: u64,
    #[arg(long, default_value_t = 2_500)]
    self_test_timeout_ms: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let raw_args: Vec<String> = std::env::args().collect();
        let normalized = normalize_args(raw_args);
        let args = Args::parse_from(normalized);

        let vid = match args.vid.as_deref() {
            Some(value) => Some(parse_u16(value).with_context(|| format!("invalid vid: {value}"))?),
            None => None,
        };
        let pid = match args.pid.as_deref() {
            Some(value) => Some(parse_u16(value).with_context(|| format!("invalid pid: {value}"))?),
            None => None,
        };

        ensure!(
            (1..=10).contains(&args.self_test_keepalives),
            "self-test keepalive count must be between 1 and 10"
        );
        ensure!(
            args.self_test_interval_ms > 0,
            "self-test interval must be greater than zero"
        );
        ensure!(
            args.self_test_timeout_ms > 0,
            "self-test timeout must be greater than zero"
        );

        Ok(Self {
            vid,
            pid,
            port: args.port,
            #[cfg(any(test, feature = "test-port-fd"))]
            port_fd: args.port_fd,
            cwd: args.cwd,
            probe_timeout_ms: args.probe_timeout_ms,
            debug_log: args.debug_log,
            send_files: args.send_file,
            raw: args.raw,
            debug: args.debug,
            device_self_test: args.device_self_test,
            self_test_keepalives: args.self_test_keepalives,
            self_test_interval_ms: args.self_test_interval_ms,
            self_test_timeout_ms: args.self_test_timeout_ms,
        })
    }
}

pub fn init_logging(debug: bool) -> Result<()> {
    let level = if debug { "debug" } else { "info" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    Ok(())
}

fn normalize_args(args: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::with_capacity(args.len());
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            if matches!(
                key,
                "vid" | "pid" | "cwd" | "probe_timeout_ms" | "send_file"
            ) {
                let arg = if key == "send_file" {
                    "--send-file".to_string()
                } else {
                    format!("--{key}")
                };
                normalized.push(arg);
                normalized.push(value.to_string());
                continue;
            }
            if matches!(key, "send-file") {
                normalized.push(format!("--{key}"));
                normalized.push(value.to_string());
                continue;
            }
            if key == "debug_log" {
                normalized.push("--debug-log".to_string());
                normalized.push(value.to_string());
                continue;
            }
            #[cfg(any(test, feature = "test-port-fd"))]
            if key == "port_fd" {
                normalized.push("--port-fd".to_string());
                normalized.push(value.to_string());
                continue;
            }
        }
        normalized.push(arg);
    }
    normalized
}

fn parse_u16(input: &str) -> Result<u16> {
    let value = input.trim();
    let (radix, digits) = if let Some(stripped) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (16, stripped)
    } else if value.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F')) {
        (16, value)
    } else {
        (10, value)
    };

    Ok(u16::from_str_radix(digits, radix)?)
}

#[cfg(test)]
mod tests {
    use super::{normalize_args, parse_u16};

    #[test]
    fn parse_hex_prefix() {
        assert_eq!(parse_u16("0x1234").unwrap(), 0x1234);
    }

    #[test]
    fn parse_hex_without_prefix() {
        assert_eq!(parse_u16("abcd").unwrap(), 0xABCD);
    }

    #[test]
    fn parse_decimal() {
        assert_eq!(parse_u16("4660").unwrap(), 4660);
    }

    #[test]
    fn normalize_send_file_kv_syntax() {
        let args = normalize_args(vec![
            "host-agent".to_string(),
            "send_file=/tmp/file.bin".to_string(),
        ]);
        assert_eq!(args[1], "--send-file");
        assert_eq!(args[2], "/tmp/file.bin");
    }
}

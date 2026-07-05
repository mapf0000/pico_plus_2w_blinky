#![cfg(target_os = "macos")]

use std::ffi::CStr;
use std::io;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::PathBuf;
use std::process::Child;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const HEADER_LEN: usize = 5;
const TAG_EXECUTE: u8 = 1;
const TAG_DEBUG_MSG: u8 = 2;
const TAG_REQUEST_AGENT_STATUS: u8 = 7;
const TAG_AGENT_STATUS: u8 = 8;
const TAG_EXECUTE_RESULT: u8 = 9;
const TAG_DB_CREDENTIALS_REQUEST: u8 = 11;
const TAG_DB_CREDENTIALS_RESPONSE: u8 = 12;
const TAG_FS_LIST_REQUEST: u8 = 29;
const TAG_FS_LIST_PAGE: u8 = 30;
const MAX_EXEC_OUTPUT: usize = 8 * 1024;
const WAIT_CONNECT_MS: u64 = 1200;
const RESPONSE_TIMEOUT_MS: u64 = 500;
const RESPONSE_DEADLINE_MS: u64 = 5000;

// Build host-agent with test-only flags once per test run.
static HOST_AGENT_BUILD: OnceLock<Option<String>> = OnceLock::new();

fn open_pty_pair() -> io::Result<(std::fs::File, std::fs::File, String)> {
    unsafe {
        let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        if libc::grantpt(master_fd) != 0 {
            return Err(io::Error::last_os_error());
        }

        if libc::unlockpt(master_fd) != 0 {
            return Err(io::Error::last_os_error());
        }

        let name_ptr = libc::ptsname(master_fd);
        if name_ptr.is_null() {
            return Err(io::Error::last_os_error());
        }

        let path = CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)?;
        set_raw(slave.as_raw_fd())?;
        clear_cloexec(slave.as_raw_fd())?;
        let master = std::fs::File::from_raw_fd(master_fd);
        Ok((master, slave, path))
    }
}

fn host_agent_cmd() -> io::Result<std::process::Command> {
    let target_dir = target_dir();
    let target = host_target_triple();
    let bin_path = if target.is_empty() {
        target_dir.join("debug").join("host-agent")
    } else {
        target_dir.join(target).join("debug").join("host-agent")
    };

    let build_error = HOST_AGENT_BUILD.get_or_init(|| {
        build_host_agent(target, &target_dir)
            .err()
            .map(|err| err.to_string())
    });
    if let Some(message) = build_error.as_ref() {
        return Err(io::Error::new(io::ErrorKind::Other, message.clone()));
    }

    if !bin_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("host-agent binary not found at {}", bin_path.display()),
        ));
    }

    Ok(std::process::Command::new(bin_path))
}

fn host_target_triple() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64-apple-darwin"
    } else {
        ""
    }
}

fn target_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|parent| parent.parent())
        .map(|root| root.join("target"))
        .unwrap_or_else(|| manifest_dir.join("target"))
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|parent| parent.parent())
        .map(|root| root.to_path_buf())
        .unwrap_or(manifest_dir)
}

fn build_host_agent(target: &str, target_dir: &PathBuf) -> io::Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = std::process::Command::new(cargo);
    cmd.current_dir(workspace_root())
        .arg("build")
        .arg("-p")
        .arg("host-agent")
        .arg("--features")
        .arg("test-port-fd");
    if !target.is_empty() {
        cmd.arg("--target").arg(target);
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        cmd.env("CARGO_TARGET_DIR", dir);
    } else {
        cmd.env("CARGO_TARGET_DIR", target_dir);
    }
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "cargo build failed for host-agent",
        ))
    }
}

fn temp_log_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("host-agent-e2e-{nanos}.log"))
}

fn temp_debug_log_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("host-agent-e2e-debug-{nanos}.log"))
}

fn load_logs(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn clear_cloexec(fd: RawFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_raw(fd: RawFd) -> io::Result<()> {
    unsafe {
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(fd, termios.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut termios = termios.assume_init();
        libc::cfmakeraw(&mut termios);
        termios.c_cflag |= libc::CLOCAL | libc::CREAD;
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn build_frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(HEADER_LEN + payload.len());
    buffer.push(tag);
    buffer.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buffer.extend_from_slice(payload);
    buffer
}

fn write_frame(file: &mut std::fs::File, tag: u8, payload: &[u8]) -> io::Result<()> {
    let frame = build_frame(tag, payload);
    file.write_all(&frame)?;
    file.flush()
}

fn read_frame(fd: RawFd, timeout: Duration) -> io::Result<(u8, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let mut header = [0u8; HEADER_LEN];
    read_exact_deadline(fd, &mut header, deadline)?;
    let tag = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    let mut payload = vec![0u8; len];
    if len > 0 {
        read_exact_deadline(fd, &mut payload, deadline)?;
    }
    Ok((tag, payload))
}

fn request_agent_status(
    master: &mut std::fs::File,
    child: &mut Child,
    log_path: &PathBuf,
) -> io::Result<(u8, Vec<u8>)> {
    request_agent_status_with_timeout(master, child, log_path, RESPONSE_DEADLINE_MS)
}

fn request_agent_status_with_timeout(
    master: &mut std::fs::File,
    child: &mut Child,
    log_path: &PathBuf,
    deadline_ms: u64,
) -> io::Result<(u8, Vec<u8>)> {
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    let mut last_error = None;

    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            let logs = load_logs(log_path);
            let message = if logs.is_empty() {
                format!("host-agent exited: {status}")
            } else {
                format!("host-agent exited: {status}\nstdout/stderr:\n{logs}")
            };
            return Err(io::Error::new(io::ErrorKind::Other, message));
        }
        if let Err(err) = write_frame(master, TAG_REQUEST_AGENT_STATUS, &[]) {
            if err.raw_os_error() == Some(libc::EIO) {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            last_error = Some(err);
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        match read_frame(
            master.as_raw_fd(),
            Duration::from_millis(RESPONSE_TIMEOUT_MS),
        ) {
            Ok(frame) => return Ok(frame),
            Err(err) => {
                if err.raw_os_error() == Some(libc::EIO) || err.kind() == io::ErrorKind::TimedOut {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                last_error = Some(err);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        let logs = load_logs(log_path);
        let message = if logs.is_empty() {
            "timeout waiting for response".to_string()
        } else {
            format!("timeout waiting for response\nstdout/stderr:\n{logs}")
        };
        io::Error::new(io::ErrorKind::TimedOut, message)
    }))
}

fn send_frame_with_retry(
    master: &mut std::fs::File,
    child: &mut Child,
    log_path: &PathBuf,
    tag: u8,
    payload: &[u8],
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(RESPONSE_DEADLINE_MS);
    let mut last_error = None;

    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            let logs = load_logs(log_path);
            let message = if logs.is_empty() {
                format!("host-agent exited: {status}")
            } else {
                format!("host-agent exited: {status}\nstdout/stderr:\n{logs}")
            };
            return Err(io::Error::new(io::ErrorKind::Other, message));
        }
        match write_frame(master, tag, payload) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if err.raw_os_error() == Some(libc::EIO) {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                last_error = Some(err);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        let logs = load_logs(log_path);
        let message = if logs.is_empty() {
            "timeout waiting to send frame".to_string()
        } else {
            format!("timeout waiting to send frame\nstdout/stderr:\n{logs}")
        };
        io::Error::new(io::ErrorKind::TimedOut, message)
    }))
}

fn read_execute_results(
    master: &mut std::fs::File,
    child: &mut Child,
    log_path: &PathBuf,
    expected_total: usize,
) -> io::Result<Vec<Vec<u8>>> {
    let deadline = Instant::now() + Duration::from_millis(RESPONSE_DEADLINE_MS);
    let mut chunks = Vec::new();
    let mut total = 0usize;

    while Instant::now() < deadline && total < expected_total {
        if let Some(status) = child.try_wait()? {
            let logs = load_logs(log_path);
            let message = if logs.is_empty() {
                format!("host-agent exited: {status}")
            } else {
                format!("host-agent exited: {status}\nstdout/stderr:\n{logs}")
            };
            return Err(io::Error::new(io::ErrorKind::Other, message));
        }
        match read_frame(
            master.as_raw_fd(),
            Duration::from_millis(RESPONSE_TIMEOUT_MS),
        ) {
            Ok((tag, payload)) => {
                if tag != TAG_EXECUTE_RESULT {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unexpected tag {tag} during execute"),
                    ));
                }
                total += payload.len();
                chunks.push(payload);
            }
            Err(err) => {
                if err.raw_os_error() == Some(libc::EIO) || err.kind() == io::ErrorKind::TimedOut {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                return Err(err);
            }
        }
    }

    if total < expected_total {
        let logs = load_logs(log_path);
        let message = if logs.is_empty() {
            format!("execute output short: got {total} bytes, expected {expected_total}")
        } else {
            format!(
                "execute output short: got {total} bytes, expected {expected_total}\nstdout/stderr:\n{logs}"
            )
        };
        return Err(io::Error::new(io::ErrorKind::TimedOut, message));
    }

    Ok(chunks)
}

fn spawn_agent(slave_fd: RawFd, debug_log: Option<&PathBuf>) -> io::Result<(Child, PathBuf)> {
    spawn_agent_with_env(slave_fd, debug_log, &[])
}

fn spawn_agent_with_env(
    slave_fd: RawFd,
    debug_log: Option<&PathBuf>,
    envs: &[(&str, &str)],
) -> io::Result<(Child, PathBuf)> {
    let mut cmd = host_agent_cmd()?;
    let log_path = temp_log_path();
    let log_file = std::fs::File::create(&log_path)?;
    let log_file_err = log_file.try_clone()?;
    cmd.arg("--port-fd")
        .arg(slave_fd.to_string())
        .arg("--debug")
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err));
    for (key, value) in envs {
        cmd.env(key, value);
    }
    if let Some(path) = debug_log {
        cmd.arg("--debug-log").arg(path);
    }
    let child = cmd.spawn()?;
    Ok((child, log_path))
}

fn wait_for_debug_log(
    child: &mut Child,
    log_path: &PathBuf,
    debug_log: &PathBuf,
    expected: &str,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(RESPONSE_DEADLINE_MS);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            let logs = load_logs(log_path);
            let message = if logs.is_empty() {
                format!("host-agent exited: {status}")
            } else {
                format!("host-agent exited: {status}\nstdout/stderr:\n{logs}")
            };
            return Err(io::Error::new(io::ErrorKind::Other, message));
        }
        if let Ok(contents) = std::fs::read_to_string(debug_log) {
            if contents.contains(expected) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let logs = load_logs(log_path);
    let message = if logs.is_empty() {
        "timeout waiting for debug log".to_string()
    } else {
        format!("timeout waiting for debug log\nstdout/stderr:\n{logs}")
    };
    Err(io::Error::new(io::ErrorKind::TimedOut, message))
}

#[test]
fn e2e_agent_status_roundtrip() -> io::Result<()> {
    let (master, slave, _slave_path) = open_pty_pair()?;
    let mut master = master;
    let slave_fd = slave.as_raw_fd();

    let (mut child, log_path) = spawn_agent(slave_fd, None)?;
    drop(slave);

    let result = (|| {
        std::thread::sleep(Duration::from_millis(WAIT_CONNECT_MS));
        request_agent_status(&mut master, &mut child, &log_path)
    })();

    let _ = child.kill();
    let _ = child.wait();

    let (tag, payload) = result?;

    assert_eq!(tag, TAG_AGENT_STATUS);
    assert!(!payload.is_empty());
    Ok(())
}

#[test]
fn e2e_execute_roundtrip() -> io::Result<()> {
    let (master, slave, _slave_path) = open_pty_pair()?;
    let mut master = master;
    let slave_fd = slave.as_raw_fd();

    let (mut child, log_path) = spawn_agent(slave_fd, None)?;
    drop(slave);

    std::thread::sleep(Duration::from_millis(WAIT_CONNECT_MS));
    send_frame_with_retry(
        &mut master,
        &mut child,
        &log_path,
        TAG_EXECUTE,
        b"printf ok",
    )?;
    let chunks = read_execute_results(&mut master, &mut child, &log_path, 2)?;

    let output: Vec<u8> = chunks.into_iter().flatten().collect();
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(output, b"ok");
    Ok(())
}

#[test]
fn e2e_execute_truncation() -> io::Result<()> {
    let (master, slave, _slave_path) = open_pty_pair()?;
    let mut master = master;
    let slave_fd = slave.as_raw_fd();

    let (mut child, log_path) = spawn_agent(slave_fd, None)?;
    drop(slave);

    std::thread::sleep(Duration::from_millis(WAIT_CONNECT_MS));
    let command = "yes a | head -c 9000";
    send_frame_with_retry(
        &mut master,
        &mut child,
        &log_path,
        TAG_EXECUTE,
        command.as_bytes(),
    )?;
    let chunks = read_execute_results(&mut master, &mut child, &log_path, MAX_EXEC_OUTPUT)?;

    let total: usize = chunks.iter().map(|chunk| chunk.len()).sum();
    for chunk in &chunks {
        assert!(chunk.len() <= 2048);
    }
    assert_eq!(total, MAX_EXEC_OUTPUT);

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

#[test]
fn e2e_db_credentials_roundtrip() -> io::Result<()> {
    let (master, slave, _slave_path) = open_pty_pair()?;
    let mut master = master;
    let slave_fd = slave.as_raw_fd();

    let envs = [
        ("HOST_AGENT_DB_USER", "dbuser"),
        ("HOST_AGENT_DB_PASSWORD", "dbpass"),
    ];
    let (mut child, log_path) = spawn_agent_with_env(slave_fd, None, &envs)?;
    drop(slave);

    std::thread::sleep(Duration::from_millis(WAIT_CONNECT_MS));
    send_frame_with_retry(
        &mut master,
        &mut child,
        &log_path,
        TAG_DB_CREDENTIALS_REQUEST,
        b"analytics-db",
    )?;
    let (tag, payload) = read_frame(
        master.as_raw_fd(),
        Duration::from_millis(RESPONSE_DEADLINE_MS),
    )?;

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(tag, TAG_DB_CREDENTIALS_RESPONSE);
    let Some(split_at) = payload.iter().position(|byte| *byte == 0) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "credential payload missing separator",
        ));
    };
    let (user_bytes, pass_bytes) = payload.split_at(split_at);
    assert_eq!(user_bytes, b"dbuser");
    assert_eq!(&pass_bytes[1..], b"dbpass");
    Ok(())
}

#[test]
fn e2e_debug_msg_logging() -> io::Result<()> {
    let (master, slave, _slave_path) = open_pty_pair()?;
    let mut master = master;
    let slave_fd = slave.as_raw_fd();

    let debug_log = temp_debug_log_path();
    let (mut child, log_path) = spawn_agent(slave_fd, Some(&debug_log))?;
    drop(slave);

    std::thread::sleep(Duration::from_millis(WAIT_CONNECT_MS));
    let message = "debug-line";
    send_frame_with_retry(
        &mut master,
        &mut child,
        &log_path,
        TAG_DEBUG_MSG,
        message.as_bytes(),
    )?;
    wait_for_debug_log(&mut child, &log_path, &debug_log, message)?;

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

#[test]
fn e2e_filesystem_listing_roundtrip() -> io::Result<()> {
    let (master, slave, _slave_path) = open_pty_pair()?;
    let mut master = master;
    let slave_fd = slave.as_raw_fd();
    let root = std::env::temp_dir().join(format!(
        "host-agent-e2e-fs-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("folder"))?;
    std::fs::write(root.join("file.bin"), b"content")?;

    let (mut child, log_path) = spawn_agent(slave_fd, None)?;
    drop(slave);

    let result = (|| {
        std::thread::sleep(Duration::from_millis(WAIT_CONNECT_MS));
        let path = root.to_string_lossy();
        let mut request = Vec::new();
        request.extend_from_slice(&1u16.to_le_bytes());
        request.extend_from_slice(&77u64.to_le_bytes());
        request.extend_from_slice(&0u32.to_le_bytes());
        request.extend_from_slice(&64u16.to_le_bytes());
        request.push(0);
        request.extend_from_slice(&(path.len() as u16).to_le_bytes());
        request.extend_from_slice(path.as_bytes());
        send_frame_with_retry(
            &mut master,
            &mut child,
            &log_path,
            TAG_FS_LIST_REQUEST,
            &request,
        )?;
        read_frame(
            master.as_raw_fd(),
            Duration::from_millis(RESPONSE_DEADLINE_MS),
        )
    })();

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);

    let (tag, payload) = result?;
    assert_eq!(tag, TAG_FS_LIST_PAGE);
    assert!(payload.len() <= 2048);
    assert_eq!(&payload[0..2], &1u16.to_le_bytes());
    assert_eq!(&payload[2..10], &77u64.to_le_bytes());
    assert_eq!(payload[10], 0);
    assert!(
        payload
            .windows(b"folder".len())
            .any(|part| part == b"folder")
    );
    assert!(
        payload
            .windows(b"file.bin".len())
            .any(|part| part == b"file.bin")
    );
    Ok(())
}

fn read_exact_deadline(fd: RawFd, buf: &mut [u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < buf.len() {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timeout waiting for data",
            ));
        }
        let timeout_ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timeout waiting for data",
            ));
        }
        if pfd.revents & libc::POLLIN == 0 {
            continue;
        }
        let read_len = unsafe {
            libc::read(
                fd,
                buf[offset..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - offset,
            )
        };
        if read_len < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if err.raw_os_error() == Some(libc::EIO) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no slave attached"));
            }
            if err.kind() == io::ErrorKind::WouldBlock {
                continue;
            }
            return Err(err);
        }
        if read_len == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pty closed"));
        }
        offset += read_len as usize;
    }
    Ok(())
}

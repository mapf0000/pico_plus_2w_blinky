use anyhow::{Context, Result, bail};
use serialport::TTYPort;
use std::ffi::CStr;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use tokio_serial::SerialStream;

pub struct Pty {
    pub master: SerialStream,
    pub slave: File,
    pub slave_path: String,
}

pub fn open() -> Result<Pty> {
    // SAFETY: every successful descriptor-producing call is immediately owned by
    // a File/TTYPort, and every C return value is checked before it is used.
    unsafe {
        let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master_fd < 0 {
            return Err(std::io::Error::last_os_error()).context("open PTY master");
        }
        if libc::grantpt(master_fd) != 0 || libc::unlockpt(master_fd) != 0 {
            let error = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(error).context("initialize PTY master");
        }
        let name = libc::ptsname(master_fd);
        if name.is_null() {
            let error = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(error).context("resolve PTY slave path");
        }
        let slave_path = CStr::from_ptr(name).to_string_lossy().into_owned();
        let slave = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&slave_path)
        {
            Ok(slave) => slave,
            Err(error) => {
                libc::close(master_fd);
                return Err(error).context("open PTY slave");
            }
        };
        if let Err(error) =
            set_raw(slave.as_raw_fd()).and_then(|()| clear_cloexec(slave.as_raw_fd()))
        {
            libc::close(master_fd);
            return Err(error);
        }

        let tty = TTYPort::from_raw_fd(master_fd);
        let master = SerialStream::try_from(tty).context("create asynchronous PTY stream")?;
        Ok(Pty {
            master,
            slave,
            slave_path,
        })
    }
}

fn clear_cloexec(fd: RawFd) -> Result<()> {
    // SAFETY: fd is a live slave PTY owned by the caller.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
            bail!(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_raw(fd: RawFd) -> Result<()> {
    // SAFETY: termios is initialized by tcgetattr before being read and fd is a
    // live terminal descriptor.
    unsafe {
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(fd, termios.as_mut_ptr()) != 0 {
            bail!(std::io::Error::last_os_error());
        }
        let mut termios = termios.assume_init();
        libc::cfmakeraw(&mut termios);
        termios.c_cflag |= libc::CLOCAL | libc::CREAD;
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            bail!(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

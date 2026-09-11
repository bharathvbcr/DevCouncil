//! Own the output handle without changing the parent's file-description flags.
//! Unix terminals are reopened and identity-checked before O_NONBLOCK writes.
//! Other streams are isolated on the renderer thread with a bounded shutdown.

use std::fs::File;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) struct Sink {
    file: File,
    #[cfg(unix)]
    inherited: Option<File>,
}

impl Sink {
    #[cfg(unix)]
    pub(super) fn stderr(terminal: bool) -> io::Result<Self> {
        use std::ffi::CStr;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        // One-shot commands restore default SIGPIPE for their primary stdout.
        // Progress is optional: a pipe reader disappearing between poll and
        // write must return EPIPE, not terminate indexing. Mask only this
        // dedicated renderer thread, whose lifetime ends with this sink.
        let mut signals = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: sigemptyset initializes the writable set before it is read.
        if unsafe { libc::sigemptyset(signals.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut signals = unsafe { signals.assume_init() };
        if unsafe { libc::sigaddset(&mut signals, libc::SIGPIPE) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let error =
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut()) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }

        // SAFETY: duplicate fd 2 atomically with CLOEXEC; never take ownership
        // of the inherited descriptor or alter its shared status flags.
        let fd = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { File::from_raw_fd(fd) };
        if !terminal {
            return Ok(Self {
                file: original,
                inherited: None,
            });
        }
        Self::check_peer(&original)?;

        let mut path = [0 as libc::c_char; 4096];
        // SAFETY: the owned fd is live and path has the stated writable length.
        let error = unsafe { libc::ttyname_r(original.as_raw_fd(), path.as_mut_ptr(), path.len()) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        // ttyname_r succeeds only with a NUL-terminated path in the buffer.
        let path = unsafe { CStr::from_ptr(path.as_ptr()) };
        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
            .open(std::ffi::OsStr::from_bytes(path.to_bytes()))?;
        let before = original.metadata()?;
        let after = file.metadata()?;
        if (before.dev(), before.ino(), before.rdev()) != (after.dev(), after.ino(), after.rdev()) {
            return Err(io::Error::other(
                "stderr terminal identity changed while reopening it",
            ));
        }
        Self::check_peer(&original)?;
        Ok(Self {
            file,
            inherited: Some(original),
        })
    }

    #[cfg(windows)]
    pub(super) fn stderr(_terminal: bool) -> io::Result<Self> {
        use std::os::windows::io::AsHandle;
        // A separate owned handle avoids the process-wide stdio lock. Windows
        // uses plain output until a bounded console animation sink is verified.
        let owned = std::io::stderr().as_handle().try_clone_to_owned()?;
        Ok(Self {
            file: File::from(owned),
        })
    }

    pub(super) fn write(&mut self, mut bytes: &[u8], stopped: &AtomicBool) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_millis(20);
        for _ in 0..16 {
            if bytes.is_empty() {
                return Ok(());
            }
            if stopped.load(Ordering::Acquire) || Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "progress output deadline",
                ));
            }
            #[cfg(unix)]
            {
                // Keep observing the inherited description: reopening a device
                // path must never erase its existing hangup or replace its peer.
                if let Some(inherited) = &self.inherited {
                    Self::check_peer(inherited)?;
                }
                Self::check_peer(&self.file)?;
            }
            match self.file.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => bytes = &bytes[n..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if bytes.is_empty() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "progress output write limit",
            ))
        }
    }

    #[cfg(unix)]
    fn check_peer(file: &File) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let mut fd = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: poll receives one valid, live owned descriptor; timeout is zero.
        if unsafe { libc::poll(&mut fd, 1, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "progress output disconnected",
            ));
        }
        if fd.revents & libc::POLLOUT == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "progress output is not writable",
            ));
        }
        Ok(())
    }
}

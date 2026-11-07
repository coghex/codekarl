//! The named pipe that holds the UI VM in the held-UI window cases (CK-206), and never blocks the supervisor.
//!
//! The UI script's `frame_update` callback reads one line from the pipe: `io.open(fifo, "r"):read("l")` blocks the UI owner thread
//! on the operating system until the supervisor writes. It is an event wait: no polling and no fixed sleep.
//!
//! **Nothing here opens or writes the pipe in blocking mode.** Before it spawns the worker the supervisor creates the FIFO and
//! opens **both ends itself**, nonblocking: first `O_RDONLY | O_NONBLOCK` (POSIX: it returns at once), then `O_WRONLY | O_NONBLOCK`
//! (it succeeds at once because a reader, its own, now exists). It holds both descriptors for the whole case and never reads from its
//! read end. So:
//!
//! - the worker's `io.open(fifo, "r")` returns at once (a writer exists), and its `read("l")` then blocks until a line arrives;
//! - the release is one nonblocking `write` of a short line (well under `PIPE_BUF`) to the held write end: into an empty pipe it
//!   completes at once, whether or not the worker ever opened the pipe, and the supervisor's own read end means no `EPIPE` and no
//!   `SIGPIPE`;
//! - the release is idempotent: it is written at most once per case, by a flag.
//!
//! On every path, a panic included, the case ends with the bounded release, then the worker is killed and reaped as the harness
//! does today, then both descriptors are closed and the FIFO is removed ([`HeldCase`]).

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The line the release writes.
const RELEASE_LINE: &[u8] = b"go\n";

/// A FIFO with both of its ends held by the supervisor.
pub(crate) struct HeldPipe {
    path: PathBuf,
    /// The supervisor's own read end; it is never read from.
    _read: File,
    write: File,
    released: bool,
}

impl HeldPipe {
    /// Creates the FIFO at `path` and opens its two ends, nonblocking (the standard library opens close-on-exec), read end first.
    /// The one `unsafe` is `mkfifo`; everything else is the standard library's safe file API.
    pub(crate) fn create(path: PathBuf) -> io::Result<Self> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path with a NUL"))?;
        // SAFETY: `c_path` is a valid NUL-terminated path that outlives the call, and `mkfifo` keeps no pointer to it.
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let opened = (|| {
            let read = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)?;
            let write = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)?;
            Ok::<_, io::Error>((read, write))
        })();
        match opened {
            Ok((read, write)) => Ok(Self {
                path,
                _read: read,
                write,
                released: false,
            }),
            Err(error) => {
                let _ = fs::remove_file(&path);
                Err(error)
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the release line once. Later calls do nothing. Never blocks; returns whether the line was written by this call.
    pub(crate) fn release(&mut self) -> bool {
        if self.released {
            return false;
        }
        self.released = true;
        // One short write into an empty pipe (well under `PIPE_BUF`): it completes at once.
        (&self.write)
            .write(RELEASE_LINE)
            .is_ok_and(|count| count == RELEASE_LINE.len())
    }

    pub(crate) fn was_released(&self) -> bool {
        self.released
    }
}

impl Drop for HeldPipe {
    fn drop(&mut self) {
        // The file goes first, so nothing can open it afterwards; the descriptors close with the fields.
        let _ = fs::remove_file(&self.path);
    }
}

/// A held pipe and the worker that reads it, ended in the fixed order on every path.
pub(crate) struct HeldCase<W> {
    pipe: HeldPipe,
    worker: Option<W>,
}

impl<W> HeldCase<W> {
    pub(crate) fn new(pipe: HeldPipe, worker: W) -> Self {
        Self {
            pipe,
            worker: Some(worker),
        }
    }

    pub(crate) fn worker(&mut self) -> &mut W {
        self.worker.as_mut().expect("the worker is still running")
    }

    pub(crate) fn pipe(&mut self) -> &mut HeldPipe {
        &mut self.pipe
    }

    /// The worker, to be finished by the case; the pipe is still released, closed and removed when this is dropped.
    pub(crate) fn take_worker(&mut self) -> W {
        self.worker.take().expect("the worker was already taken")
    }
}

impl<W> Drop for HeldCase<W> {
    fn drop(&mut self) {
        // 1. The bounded release (nonblocking, at most once). 2. The worker is killed and reaped by its own drop. 3. The pipe's
        // descriptors close and the FIFO is removed when the `pipe` field drops after this body.
        self.pipe.release();
        drop(self.worker.take());
    }
}

/// The camera lines of a map observation: `camera <step> <x> <y>`.
pub(crate) fn cameras(observation: &str) -> Vec<(i64, i64, i64)> {
    observation
        .lines()
        .filter_map(|line| {
            let mut parts = line.strip_prefix("camera ")?.split(' ');
            let step = parts.next()?.parse().ok()?;
            let x = parts.next()?.parse().ok()?;
            let y = parts.next()?.parse().ok()?;
            Some((step, x, y))
        })
        .collect()
}

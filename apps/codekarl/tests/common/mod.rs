//! Helpers shared by the end-to-end tests of the built binary (`runtime.rs` and `lua_vms.rs`): guarded process
//! and pipe creation, running the binary with a deadline, scratch directories, and a full pipe.
//!
//! The only `unsafe` here is the minimal `fcntl` calls in `set_blocking` and `set_cloexec` on test-owned descriptors.

use std::{
    ffi::OsStr,
    fs,
    io::{self, PipeReader, PipeWriter, Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

pub(crate) const BINARY: &str = env!("CARGO_BIN_EXE_codekarl");
/// The longest any ordinary child may run.
pub(crate) const CHILD_DEADLINE: Duration = Duration::from_secs(30);
/// The longest a child with a stalled or closed stdout may take to exit with status 3.
pub(crate) const STALLED_DEADLINE: Duration = Duration::from_secs(10);

static SPAWN_LOCK: Mutex<()> = Mutex::new(());
static WAITING_FOR_SPAWN_LOCK: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());

/// Holds this test executable's process/pipe creation lock.
pub(crate) struct SpawnGuard {
    _lock: MutexGuard<'static, ()>,
}

pub(crate) fn spawn_guard() -> SpawnGuard {
    let id = thread::current().id();
    WAITING_FOR_SPAWN_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(id);
    debug_assert!(
        is_waiting(id),
        "the waiting thread is registered before it blocks"
    );
    let lock = SPAWN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    WAITING_FOR_SPAWN_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|waiting| *waiting != id);
    SpawnGuard { _lock: lock }
}

/// Whether `id` is waiting for this executable's process/pipe creation lock.
pub(crate) fn is_waiting(id: ThreadId) -> bool {
    WAITING_FOR_SPAWN_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&id)
}

/// The only raw `Command::spawn` call in this executable.
pub(crate) fn spawn_locked(_held: &SpawnGuard, command: &mut Command) -> io::Result<Child> {
    command.spawn()
}

/// The only raw `std::io::pipe` call in this executable.
pub(crate) fn pipe_locked(_held: &SpawnGuard) -> io::Result<(PipeReader, PipeWriter)> {
    io::pipe()
}

/// What a child did.
#[derive(Debug)]
pub(crate) struct Run {
    /// The exit code, or `None` when it was killed (by the deadline or by a signal).
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
}

fn drain<R: Read + Send + 'static>(stream: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// As [`run_with`], in the directory `cwd` (the test's own when `None`).
pub(crate) fn run_in(
    cwd: Option<&Path>,
    args: &[&OsStr],
    stdout: Option<Stdio>,
    deadline: Duration,
) -> Run {
    run_in_with_spawn_notice(cwd, args, stdout, deadline, || {})
}

pub(crate) fn run_in_with_spawn_notice(
    cwd: Option<&Path>,
    args: &[&OsStr],
    stdout: Option<Stdio>,
    deadline: Duration,
    spawned: impl FnOnce(),
) -> Run {
    let own_stdout = stdout.is_none();
    let mut command = command_for(cwd, args, stdout);
    let child = spawn_locked(&spawn_guard(), &mut command).expect("the binary starts");
    drop(command);
    spawned();
    finish_child(child, own_stdout, deadline)
}

pub(crate) fn command_for(cwd: Option<&Path>, args: &[&OsStr], stdout: Option<Stdio>) -> Command {
    let mut command = Command::new(BINARY);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.stdout(stdout.unwrap_or_else(Stdio::piped));
    command
}

pub(crate) fn finish_child(mut child: Child, own_stdout: bool, deadline: Duration) -> Run {
    let out = drain(own_stdout.then(|| child.stdout.take()).flatten());
    let err = drain(child.stderr.take());
    let give_up = Instant::now() + deadline;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().expect("the child can be polled") {
            break (status, false);
        }
        if Instant::now() >= give_up {
            let _ = child.kill();
            break (child.wait().expect("the killed child is reaped"), true);
        }
        thread::sleep(Duration::from_millis(5));
    };
    Run {
        code: status.code(),
        stdout: out.join().expect("stdout reader"),
        stderr: err.join().expect("stderr reader"),
        timed_out,
    }
}

/// A fresh directory for fixture files, removed on drop.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "codekarl-runtime-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    /// Writes `text` to `name` in the directory and returns the absolute path.
    pub(crate) fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("a fixture file");
        assert!(path.is_absolute(), "{}", path.display());
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Sets the descriptor's blocking mode. The only `unsafe` in this file: two minimal `fcntl` calls on a
/// descriptor the test owns through an `OwnedFd`.
fn set_blocking(fd: &OwnedFd, blocking: bool) {
    // SAFETY: `fd` is a valid descriptor owned by the caller for the duration of the call, and
    // `F_GETFL` reads its flags and nothing else.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1, "{}", io::Error::last_os_error());
    let wanted = if blocking {
        flags & !libc::O_NONBLOCK
    } else {
        flags | libc::O_NONBLOCK
    };
    // SAFETY: as above; `F_SETFL` changes the flags of the descriptor and nothing else.
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, wanted) };
    assert_ne!(result, -1, "{}", io::Error::last_os_error());
}

/// Sets whether a descriptor is closed by `exec`.
pub(crate) fn set_cloexec(fd: &OwnedFd, enabled: bool) {
    // SAFETY: `fd` is a valid descriptor owned by the caller for the duration of the call, and
    // `F_GETFD` reads its descriptor flags and nothing else.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(flags, -1, "{}", io::Error::last_os_error());
    let wanted = if enabled {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: `fd` remains valid and owned by the caller, and `F_SETFD` changes only its descriptor flags.
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, wanted) };
    assert_ne!(result, -1, "{}", io::Error::last_os_error());
}

/// A pipe whose write end is completely full and whose read end stays open and is never read.
pub(crate) fn full_pipe() -> (OwnedFd, OwnedFd) {
    let _ = set_cloexec;
    let (reader, writer) = pipe_locked(&spawn_guard()).expect("a pipe");
    let writer = OwnedFd::from(writer);
    set_blocking(&writer, false);
    let mut file = fs::File::from(writer.try_clone().expect("a duplicate descriptor"));
    let chunk = [b'x'; 4096];
    loop {
        match file.write(&chunk) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("filling the pipe failed: {error}"),
        }
    }
    drop(file);
    // The child's end is blocking again, as it would be: it blocks in its first write.
    set_blocking(&writer, true);
    (OwnedFd::from(reader), writer)
}

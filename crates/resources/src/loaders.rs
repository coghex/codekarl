//! The loader pool: a fixed number of threads that run the service's jobs and retirement work as they arrive, and a
//! bounded end (`docs/scene-design.md` §3, §6.2, §8.3).
//!
//! # The threads
//!
//! [`LoaderPool::start`] spawns `decode_workers` threads named `ck-loader-<n>`. Each loops: retirement work pending →
//! [`ResourceService::maintain`] (64 resources a step); else a job → run it (a panic in it is caught inside
//! [`Job::run`](crate::Job::run)) → [`ResourceService::complete`]; else it **sleeps on the service's condition variable**,
//! in [`ResourceService::wait_for_work`], by the runnable predicate documented there. It is woken when a job is queued,
//! a resource becomes retiring, CPU bytes are returned while a deferred job waits for them, and at shutdown. **No wake is
//! lost**: the predicate is evaluated under the service lock and every notifier changes the state it reads under that
//! lock or takes it before notifying. **No thread polls or sleeps on a timer.** No lock is held while a job runs, a
//! file is read or an image is decoded.
//!
//! A loader's whole loop runs inside its own outer `catch_unwind`: a panic outside a job (in the loop, `maintain` or the
//! wait) is recorded, its payload is disposed of **on that loader thread** (so none is left in the thread's result for a
//! joiner, or for the standard library's detached-thread clean-up, to drop; the pinned Rust aborts if dropping an
//! unconsumed thread result panics), and the thread returns normally, claiming no further work. This is a root boundary,
//! not a claim to catch aborts or native failures. The application's panic and resource-latch policy for the pool is the
//! composition's decision.
//!
//! # A bounded end
//!
//! [`LoaderPool::shutdown`] forms one absolute deadline (now plus `within`), marks the pool stopping, closes the service
//! ([`ResourceService::close`]), wakes every thread, then observes each thread **finished** against the deadline. A
//! running decode is not interrupted (cancellation is advisory): the thread ends after that job.
//!
//! **The cost.** `close` is synchronous: it is O(registry capacity) plus O(queued and waiting work), the freeing of
//! those buffers is paid on the caller's thread, and no wall-clock bound is claimed for it. It is not interrupted, and
//! its time counts against the deadline; if it uses the whole deadline, each thread gets one non-waiting observation.
//! So `shutdown` returns after the **larger of `close`'s cost and `within`**, plus a small scheduling bound: the deadline
//! bounds the *waiting for threads*, not `close`'s work. The composition budgets for that within the owner's deadline.
//! [`LoaderShutdown`] reports both times. Dropping a pool without `shutdown` runs `close` and one non-waiting
//! observation, which is `shutdown` with a zero budget, with the same cost.
//!
//! A thread is *finished* when its first-registered thread-local guard has run: thread-local destructors run in reverse
//! order of registration, so the guard runs after every other thread-local destructor, a panicking one included; only
//! the operating system's exit remains, so the join that follows cannot be held by anything a job or the decoder owns.
//! A thread not finished by the deadline is **detached, not joined**; its later result is discarded: a late
//! [`complete`](ResourceService::complete) returns only its job slot, job metadata and CPU reservations, never queues an
//! upload, changes a status, shows content or reopens admission.
//!
//! # The limit (§8.3)
//!
//! In a headless run the same pool runs CPU and file jobs, statuses reach `cpu_ready` or `failed`, and the run's end is
//! `shutdown` then [`ResourceService::finish_teardown`] under the caller's owner deadline. **The error of a file that
//! had not finished loading when the run ended is never observed**: the run does not wait for optional textures.

use std::cell::RefCell;
use std::fmt;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::service::{ResourceService, Work};
use crate::unwind;

/// Retiring resources one maintenance step handles.
const RETIREMENT_STEP: usize = 64;

/// The fixed budget in which the threads of a pool that could not be fully started are stopped and observed finished:
/// the runtime's clean-up deadline default (`crates/runtime/src/lib.rs`), one second.
const SPAWN_CLEANUP: Duration = Duration::from_secs(1);

/// A loader thread that ended by a failure of its loop outside any job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopFailure {
    /// The thread's name.
    pub thread: String,
    /// The panic's text (bounded), or a fixed text for a payload that is not text.
    pub message: String,
}

/// What a bounded end found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderShutdown {
    /// The threads observed finished (and joined), by name.
    pub finished: Vec<String>,
    /// The threads not finished by the deadline (detached, not joined), by name.
    pub unfinished: Vec<String>,
    /// The threads that ended by a failure of their loop.
    pub loop_failures: Vec<LoopFailure>,
    /// How long the service's `close` took: its synchronous cost, which is not bounded by the deadline.
    pub closed_in: Duration,
    /// How long the observation of the threads took, against the deadline's remainder.
    pub waited: Duration,
}

/// Why a pool could not be started. The threads already started were stopped and observed finished within a fixed
/// clean-up budget; any not finished by then were detached and are named.
#[derive(Debug)]
pub struct SpawnError {
    /// Which thread (zero-based) could not be started.
    pub index: usize,
    /// Why.
    pub source: io::Error,
    /// The threads that were started and had not finished when the clean-up budget ended.
    pub unfinished: Vec<String>,
}

impl fmt::Display for SpawnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot start loader thread {}: {}",
            self.index, self.source
        )?;
        if !self.unfinished.is_empty() {
            write!(
                formatter,
                "; not finished after the clean-up budget (detached): {}",
                self.unfinished.join(", ")
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for SpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// A thread's real exit, signalled by its first-registered thread-local guard.
struct Exit {
    done: Mutex<bool>,
    changed: Condvar,
}

impl Exit {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Mutex::new(false),
            changed: Condvar::new(),
        })
    }

    /// Waits until the thread has exited or `deadline` passes (one non-waiting look if it has passed); whether it exited.
    fn wait_until(&self, deadline: Instant) -> bool {
        let mut done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            done = self
                .changed
                .wait_timeout(done, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// Sets the exit flag when it is dropped: the loader thread's exit signal.
struct ExitSignal(Arc<Exit>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        *self.0.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.changed.notify_all();
    }
}

thread_local! {
    /// The loader's exit guard, registered by the thread's **first action** (the first access of a thread-local with a
    /// destructor registers that destructor); thread-local destructors run in reverse order of registration, so it
    /// runs last.
    static EXIT_GUARD: RefCell<Option<ExitSignal>> = const { RefCell::new(None) };
}

/// One loader thread.
struct Loader {
    name: String,
    exit: Arc<Exit>,
    handle: Option<JoinHandle<()>>,
}

type Failures = Arc<Mutex<Vec<LoopFailure>>>;

/// The fixed pool of loader threads of one [`ResourceService`].
pub struct LoaderPool {
    service: ResourceService,
    loaders: Vec<Loader>,
    failures: Failures,
    ended: bool,
}

/// How a thread is started (a failing one in the tests).
type Spawner<'a> = &'a dyn Fn(usize, Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>>;

impl LoaderPool {
    /// Starts `decode_workers` loader threads (the service's [`ResourceLimits`](crate::ResourceLimits) field) named
    /// `ck-loader-<n>`.
    ///
    /// # Errors
    ///
    /// [`SpawnError`] if a thread cannot be started: the ones already started are stopped and observed finished within a
    /// fixed clean-up budget (one second), and any not finished by then are detached and named in the error.
    pub fn start(service: ResourceService) -> Result<Self, SpawnError> {
        Self::start_with(service, &|index, body| {
            thread::Builder::new()
                .name(format!("ck-loader-{index}"))
                .spawn(body)
        })
    }

    fn start_with(service: ResourceService, spawn: Spawner<'_>) -> Result<Self, SpawnError> {
        service.allow_loaders();
        let workers = usize::try_from(service.limits().decode_workers).unwrap_or(usize::MAX);
        let failures: Failures = Arc::default();
        let mut pool = Self {
            service,
            loaders: Vec::with_capacity(workers),
            failures: Arc::clone(&failures),
            ended: false,
        };
        for index in 0..workers {
            let name = format!("ck-loader-{index}");
            let exit = Exit::new();
            let body: Box<dyn FnOnce() + Send> = {
                let (service, exit, failures, name) = (
                    pool.service.clone(),
                    Arc::clone(&exit),
                    Arc::clone(&failures),
                    name.clone(),
                );
                Box::new(move || loader_main(&name, &service, exit, &failures))
            };
            match spawn(index, body) {
                Ok(handle) => pool.loaders.push(Loader {
                    name,
                    exit,
                    handle: Some(handle),
                }),
                Err(source) => {
                    pool.service.stop_loaders();
                    let (_, unfinished) = pool.observe(Instant::now() + SPAWN_CLEANUP);
                    pool.ended = true;
                    return Err(SpawnError {
                        index,
                        source,
                        unfinished,
                    });
                }
            }
        }
        Ok(pool)
    }

    /// How many loader threads the pool started.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.loaders.len()
    }

    /// Observes each thread against `deadline`: `(finished, unfinished)` names. A finished thread is joined; one still
    /// running is detached (its handle dropped, never joined).
    fn observe(&mut self, deadline: Instant) -> (Vec<String>, Vec<String>) {
        let (mut finished, mut unfinished) = (Vec::new(), Vec::new());
        for loader in std::mem::take(&mut self.loaders) {
            let Loader { name, exit, handle } = loader;
            if exit.wait_until(deadline) {
                // Only the operating system's exit remains, so this join cannot be held.
                if let Some(Err(payload)) = handle.map(JoinHandle::join) {
                    unwind::dispose(payload);
                }
                finished.push(name);
            } else {
                drop(handle);
                unfinished.push(name);
            }
        }
        (finished, unfinished)
    }

    /// The bounded end: marks the pool stopping, closes the service, wakes every thread, then observes each against
    /// one absolute deadline of `within` from now (see the module documentation for what is and is not bounded).
    pub fn shutdown(&mut self, within: Duration) -> LoaderShutdown {
        let deadline = Instant::now() + within;
        self.service.stop_loaders();
        let closing = Instant::now();
        self.service.close();
        let closed_in = closing.elapsed();
        let waiting = Instant::now();
        let (finished, unfinished) = self.observe(deadline);
        let waited = waiting.elapsed();
        self.ended = true;
        LoaderShutdown {
            finished,
            unfinished,
            loop_failures: self
                .failures
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            closed_in,
            waited,
        }
    }
}

impl Drop for LoaderPool {
    fn drop(&mut self) {
        if !self.ended {
            let _ = self.shutdown(Duration::ZERO);
        }
    }
}

impl fmt::Debug for LoaderPool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoaderPool")
            .field("threads", &self.loaders.len())
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

/// A loader thread's body: the exit guard first, then the loop inside its outer boundary.
fn loader_main(name: &str, service: &ResourceService, exit: Arc<Exit>, failures: &Failures) {
    // First of all, before anything can touch another thread-local.
    EXIT_GUARD.with(|slot| *slot.borrow_mut() = Some(ExitSignal(exit)));
    let outcome = catch_unwind(AssertUnwindSafe(|| run_loop(service)));
    if let Err(payload) = outcome {
        // The known failure is published first, with its bounded message and no lock held afterwards: disposing of
        // the payload runs arbitrary `Drop` code that may stall, and the composition must not lose a failure already
        // caught because of it.
        let message = unwind::message(&payload);
        service.count_loop_failure();
        failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(LoopFailure {
                thread: name.to_owned(),
                message,
            });
        unwind::dispose(payload);
    }
}

fn run_loop(service: &ResourceService) {
    loop {
        #[cfg(test)]
        service.at_loop_top();
        match service.wait_for_work() {
            Work::Stop => return,
            Work::Retire => {
                let _ = service.maintain(RETIREMENT_STEP);
            }
            Work::Job(job) => {
                let output = job.run();
                service.complete(output);
            }
        }
    }
}

#[cfg(test)]
mod tests;

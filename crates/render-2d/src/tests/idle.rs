//! A sleeping loop: `ck_window::run` over a fake native backend whose pump sleeps until it is woken, so a test can
//! publish while the loop is idle (or just before it arms its wait) and watch what reaches the main thread with no
//! input event, redraw or UI answer.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ck_window::{
    Backend, CreatedWindow, LoopEnd, LoopSettings, NativeEvent, NativeWake, Notifier, PixelSize,
    PostError, Requests, UiAnswer, UiEvent, UiLink, Wait, WindowDesc, run,
};

use super::support::{desc, lock};
use crate::{Gpu2d, Renderer, SceneBridge};

/// The longest a fake waits for something that must happen.
pub(super) const BOUND: Duration = Duration::from_secs(10);

/// The sticky native wake and the count of sleeps.
#[derive(Default)]
pub(super) struct Native {
    woken: Mutex<bool>,
    changed: Condvar,
    sleeping: AtomicUsize,
}

impl NativeWake for Native {
    fn wake(&self) {
        *lock(&self.woken) = true;
        self.changed.notify_all();
    }
}

impl Native {
    /// Waits (bounded) until the pump has gone to sleep `n` times in all.
    pub(super) fn wait_sleeping(&self, n: usize) {
        let give_up = Instant::now() + BOUND;
        while self.sleeping.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < give_up, "the pump never went to sleep");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// What the test does at the entry of every sleeping pump, before it looks at the wake.
pub(super) type SleepHook = Box<dyn FnMut() + Send>;

struct IdleBackend {
    native: Arc<Native>,
    on_sleep: Option<SleepHook>,
}

impl Backend for IdleBackend {
    type Handles = u32;

    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<u32>, String> {
        Ok(CreatedWindow {
            handles: 7,
            pixel_size: PixelSize {
                width: desc.width,
                height: desc.height,
            },
        })
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        Arc::clone(&self.native) as Arc<dyn NativeWake>
    }

    fn pump(&mut self, _events: &mut dyn FnMut(NativeEvent), wait: Wait, _cap: usize) {
        if wait != Wait::UntilEvent {
            return;
        }
        // Entering a sleeping pump: the moment "just before the wait arms".
        if let Some(hook) = &mut self.on_sleep {
            hook();
        }
        let give_up = Instant::now() + BOUND;
        let mut woken = lock(&self.native.woken);
        self.native.sleeping.fetch_add(1, Ordering::SeqCst);
        while !*woken {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            woken = self
                .native
                .changed
                .wait_timeout(woken, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        *woken = false;
    }

    fn destroy_window(&mut self) {}

    fn terminate(&mut self) {}
}

struct IdleUi;

impl UiLink for IdleUi {
    fn post(&mut self, _event: UiEvent) -> Result<(), PostError> {
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        None
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

/// A loop running on its own thread over a [`Gpu2d`] with `u32` handles, and the means to stop it.
pub(super) struct IdleLoop {
    pub(super) native: Arc<Native>,
    notifier: Arc<Notifier>,
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<LoopEnd>>,
}

impl IdleLoop {
    /// Starts the loop. `make` builds the backend from the loop's wake; `bridge` is the renderer's scene bridge;
    /// `on_sleep` runs at the entry of every sleeping pump.
    pub(super) fn start<G>(
        bridge: &Arc<SceneBridge>,
        make: impl FnOnce(Arc<dyn Fn() + Send + Sync>) -> G + Send + 'static,
        on_sleep: Option<SleepHook>,
    ) -> Self
    where
        G: Gpu2d<Handles = u32> + 'static,
    {
        let native = Arc::new(Native::default());
        let notifier = Notifier::new();
        let quit = Arc::new(AtomicBool::new(false));
        let thread = {
            let (native, notifier, quit, bridge) = (
                Arc::clone(&native),
                Arc::clone(&notifier),
                Arc::clone(&quit),
                Arc::clone(bridge),
            );
            thread::spawn(move || {
                let requests = Requests::new();
                let writer = requests.writer(Arc::clone(&notifier));
                writer.open(desc(100, 100)).expect("a valid description");
                let wake = {
                    let notifier = Arc::clone(&notifier);
                    Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
                };
                let renderer = Renderer::new(make(wake), bridge);
                run(
                    IdleBackend { native, on_sleep },
                    renderer,
                    IdleUi,
                    &requests.reader(),
                    &notifier,
                    &quit,
                    &LoopSettings::default(),
                )
            })
        };
        Self {
            native,
            notifier,
            quit,
            thread: Some(thread),
        }
    }

    /// Waits (bounded) for the loop to end by itself and returns how, or `None` if it was still running at the
    /// bound (it is then told to quit, so its thread does not outlive the test).
    pub(super) fn end_within(mut self, bound: Duration) -> Option<LoopEnd> {
        let give_up = Instant::now() + bound;
        let thread = self.thread.take().expect("the loop thread");
        while !thread.is_finished() {
            if Instant::now() >= give_up {
                self.quit.store(true, Ordering::SeqCst);
                self.notifier.wake();
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
        Some(thread.join().expect("the loop thread finished"))
    }

    /// Stops the loop and returns how it ended.
    pub(super) fn stop(mut self) -> LoopEnd {
        self.quit.store(true, Ordering::SeqCst);
        self.notifier.wake();
        self.thread
            .take()
            .expect("the loop thread")
            .join()
            .expect("the loop thread finished")
    }
}

impl Drop for IdleLoop {
    fn drop(&mut self) {
        // A test that failed before `stop`: end the loop so its thread does not outlive the test.
        self.quit.store(true, Ordering::SeqCst);
        self.notifier.wake();
    }
}

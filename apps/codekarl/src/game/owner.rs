//! The simulation owner: the game on its own thread, as a `ck_runtime::Owner` named `sim`.
//!
//! The thread is thin. It waits on its control channel until the next tick is due (or, paused, without a timeout), reads the
//! time since the owner started, and hands both to the [`Core`]. It runs no tick for `cancel`, finishes the tick it is running
//! for `request_stop`, and never autosaves.

use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use ck_runtime::{Owner, OwnerError, Progress};
use ck_sim::Canonical;

use super::{
    core::{Core, Flow, Message},
    gameplay::Gameplay,
    slot::SimSlot,
};

/// A flag that is set once and awaited with a budget.
#[derive(Default)]
struct Latch {
    set: Mutex<bool>,
    changed: Condvar,
}

impl Latch {
    fn set(&self) {
        *self.set.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    /// Waits at most `within`; true once the latch is set.
    fn wait(&self, within: Duration) -> bool {
        let guard = self.set.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, within, |set| !*set)
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }
}

/// Sets the latch when dropped, so a thread that panics is observed finished too.
struct SetOnDrop(Arc<Latch>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// The owner's end of a running simulation thread.
pub(crate) struct SimOwner {
    name: String,
    sender: Sender<Message>,
    ready: Arc<Latch>,
    finished: Arc<Latch>,
    slot: Arc<SimSlot>,
    handle: Option<JoinHandle<()>>,
}

impl SimOwner {
    /// Starts the thread `ck-sim` running `core` (built by `build` on that thread) and returns the owner without waiting for it.
    ///
    /// `receiver` is the control channel whose sender the UI link holds.
    #[cfg(test)]
    pub(crate) fn spawn<S, C>(
        sender: Sender<Message>,
        receiver: Receiver<Message>,
        slot: Arc<SimSlot>,
        build: impl FnOnce() -> Core<S, C> + Send + 'static,
    ) -> Result<Self, String>
    where
        S: Canonical + Clone + Send + 'static,
        C: Canonical + Send + 'static,
    {
        Self::spawn_with_gameplay(sender, receiver, slot, build, None)
    }

    pub(crate) fn spawn_with_gameplay<S, C>(
        sender: Sender<Message>,
        receiver: Receiver<Message>,
        slot: Arc<SimSlot>,
        build: impl FnOnce() -> Core<S, C> + Send + 'static,
        gameplay: Option<Arc<Gameplay>>,
    ) -> Result<Self, String>
    where
        S: Canonical + Clone + Send + 'static,
        C: Canonical + Send + 'static,
    {
        let ready = Arc::new(Latch::default());
        let finished = Arc::new(Latch::default());
        let thread_ready = Arc::clone(&ready);
        let guard = SetOnDrop(Arc::clone(&finished));
        let handle = thread::Builder::new()
            .name("ck-sim".to_owned())
            .spawn(move || {
                let _finished = guard;
                let _gameplay_release = GameplayRelease(gameplay);
                let mut core = build();
                thread_ready.set();
                serve(&mut core, &receiver);
            })
            .map_err(|error| format!("cannot start the simulation thread: {error}"))?;
        Ok(Self {
            name: "sim".to_owned(),
            sender,
            ready,
            finished,
            slot,
            handle: Some(handle),
        })
    }

    /// The failure the simulation recorded, as the owner's failure.
    fn failure(&self) -> Option<OwnerError> {
        self.slot
            .failure()
            .map(|message| OwnerError::Failed { message })
    }

    fn join_if_finished(&mut self) {
        if self.finished.wait(Duration::ZERO)
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
    }
}

struct GameplayRelease(Option<Arc<Gameplay>>);

impl Drop for GameplayRelease {
    fn drop(&mut self) {
        if let Some(gameplay) = &self.0 {
            gameplay.simulation_ended();
        }
    }
}

/// The thread's loop: wait for a control or the next tick, read the time, run the core.
fn serve<S, C>(core: &mut Core<S, C>, receiver: &Receiver<Message>)
where
    S: Canonical + Clone + Send + 'static,
    C: Canonical + Send + 'static,
{
    let began = Instant::now();
    loop {
        let message = match core.wait_hint() {
            Some(limit) => match receiver.recv_timeout(limit) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match receiver.recv() {
                Ok(message) => Some(message),
                Err(_) => return,
            },
        };
        if let Some(message) = message
            && core.dispatch(message, began.elapsed()) == Flow::End
        {
            return;
        }
        let mut poll = || receiver.try_recv().ok();
        if core.run_wake(began.elapsed(), &mut poll) == Flow::End {
            return;
        }
    }
}

impl Owner for SimOwner {
    fn name(&self) -> &str {
        &self.name
    }

    fn wait_ready(&mut self, within: Duration) -> Result<(), OwnerError> {
        if self.ready.wait(within) {
            Ok(())
        } else {
            Err(OwnerError::Deadline)
        }
    }

    fn poll_ready(&mut self, within: Duration) -> Result<Progress, OwnerError> {
        Ok(if self.ready.wait(within) {
            Progress::Done
        } else {
            Progress::Pending
        })
    }

    /// The game is paused until the player resumes it, which can happen only in the run phase: there is nothing to start.
    fn request_start(&mut self) -> Result<(), OwnerError> {
        Ok(())
    }

    fn await_started(&mut self, _within: Duration) -> Result<(), OwnerError> {
        Ok(())
    }

    fn poll_started(&mut self, _within: Duration) -> Result<Progress, OwnerError> {
        Ok(Progress::Done)
    }

    /// Never blocks: the thread finishes the tick it is running and then ends.
    fn request_stop(&mut self) -> Result<(), OwnerError> {
        let _ = self.sender.send(Message::Stop);
        Ok(())
    }

    fn await_stopped(&mut self, within: Duration) -> Result<(), OwnerError> {
        if !self.finished.wait(within) {
            return Err(OwnerError::Deadline);
        }
        self.join_if_finished();
        self.failure().map_or(Ok(()), Err)
    }

    /// Ends the thread without running a tick; never blocks.
    fn cancel(&mut self) {
        let _ = self.sender.send(Message::Cancel);
    }

    fn wait_finished(&mut self, within: Duration) -> bool {
        let finished = self.finished.wait(within);
        if finished {
            self.join_if_finished();
        }
        finished
    }
}

/// A control channel for a new simulation owner.
pub(crate) fn channel() -> (Sender<Message>, Receiver<Message>) {
    mpsc::channel()
}

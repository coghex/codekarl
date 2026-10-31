//! A hold on file jobs for the tests of the crates built on the service (feature `test-support`; compiled into no
//! ordinary build).
//!
//! A test names a path with [`JobGate::hold`]; the file job for that path then waits in the gate, before it opens the
//! path, until [`JobGate::release`] (or [`JobGate::release_all`]). Nothing here is consulted by a service that was not
//! given a gate ([`ResourceService::set_job_gate`](crate::ResourceService::set_job_gate)), and with the feature off the
//! crate has no gate at all.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// The held paths and the paths whose job is waiting now.
#[derive(Debug, Default)]
struct State {
    held: HashSet<String>,
    waiting: HashSet<String>,
}

/// Holds the file jobs of named paths until the test lets them go.
#[derive(Debug, Default)]
pub struct JobGate {
    state: Mutex<State>,
    changed: Condvar,
}

impl JobGate {
    /// A gate holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The file job for `name` waits, from now, before it opens the path.
    pub fn hold(&self, name: &str) {
        self.lock().held.insert(name.to_owned());
    }

    /// Lets the job for `name` (waiting now or arriving later) go on.
    pub fn release(&self, name: &str) {
        self.lock().held.remove(name);
        self.changed.notify_all();
    }

    /// Lets every held job go on.
    pub fn release_all(&self) {
        self.lock().held.clear();
        self.changed.notify_all();
    }

    /// Whether the job for `name` is waiting in the gate now.
    #[must_use]
    pub fn is_waiting(&self, name: &str) -> bool {
        self.lock().waiting.contains(name)
    }

    /// The job side: waits while `name` is held.
    pub(crate) fn pass(&self, name: &str) {
        let mut state = self.lock();
        if !state.held.contains(name) {
            return;
        }
        state.waiting.insert(name.to_owned());
        while state.held.contains(name) {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.waiting.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::JobGate;
    use crate::{Assets, Mode, ResourceLimits, ResourceService, ServiceWakes, StatusState};

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let started = Instant::now();
        while !condition() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "gave up waiting for {what}"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn a_held_name_waits_until_released_and_other_names_pass_at_once() {
        let gate = Arc::new(JobGate::new());
        gate.hold("a.png");
        // Another name never waits.
        gate.pass("b.png");
        let job = {
            let gate = Arc::clone(&gate);
            thread::spawn(move || gate.pass("a.png"))
        };
        wait_until("the job to wait in the gate", || gate.is_waiting("a.png"));
        assert!(!job.is_finished(), "the held job has not gone on");
        gate.release("a.png");
        job.join().expect("the job went on");
        assert!(!gate.is_waiting("a.png"));
    }

    #[test]
    fn a_release_before_the_job_arrives_lets_it_through_and_release_all_frees_every_name() {
        let gate = Arc::new(JobGate::new());
        gate.hold("early.png");
        gate.release("early.png");
        gate.pass("early.png");
        gate.hold("one.png");
        gate.hold("two.png");
        let jobs: Vec<_> = ["one.png", "two.png"]
            .into_iter()
            .map(|name| {
                let gate = Arc::clone(&gate);
                thread::spawn(move || gate.pass(name))
            })
            .collect();
        wait_until("both jobs to wait", || {
            gate.is_waiting("one.png") && gate.is_waiting("two.png")
        });
        gate.release_all();
        for job in jobs {
            job.join().expect("the job went on");
        }
    }

    #[test]
    fn a_service_with_a_gate_does_not_open_a_held_path_until_it_is_released() {
        // A held file job waits before it opens its path: with the file absent it is not even failed until released
        // (then it fails, reading the path), and a name the gate does not hold runs at once.
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            Mode::Headless,
            ServiceWakes::default(),
            Some(Assets::new(
                std::env::temp_dir().join("ck-jobgate-absent-root"),
            )),
        )
        .expect("a service");
        let gate = Arc::new(JobGate::new());
        service.set_job_gate(Arc::clone(&gate));
        gate.hold("held.png");
        let held = service.load("held.png").expect("admit the held file");
        let free = service.load("free.png").expect("admit the free file");
        // One thread per job: the held one blocks its thread in the gate.
        let mut runners = Vec::new();
        while let Some(job) = service.take_job() {
            let service = service.clone();
            runners.push(thread::spawn(move || service.complete(job.run())));
        }
        assert_eq!(runners.len(), 2);
        wait_until("the free job to settle", || {
            service
                .status(free)
                .is_ok_and(|status| status.state == StatusState::Failed)
        });
        wait_until("the held job to wait in the gate", || {
            gate.is_waiting("held.png")
        });
        assert_eq!(
            service.status(held).expect("held status").state,
            StatusState::Decoding,
            "the held job has started and is waiting before it opens its path"
        );
        gate.release("held.png");
        wait_until("the released job to settle", || {
            service
                .status(held)
                .is_ok_and(|status| status.state == StatusState::Failed)
        });
        for runner in runners {
            runner.join().expect("the runner ended");
        }
        service.close();
        let _ = service.finish_teardown();
    }
}

//! CK-108: the CPU reservation of a file job, the deferred queue and the capacity-change handshake (`docs/scene-design.md`
//! §7.3), with real loader threads held at the points of the protocol by latches.
//!
//! The rig: a 64 MiB CPU budget; *holders* are Rust resources whose finished revisions the fake backend keeps (their
//! bytes are in transit and return when the backend drops them); *files* are 4096 x 2048 images, 32 MiB decoded. A file
//! is refused while the holders leave less than 32 MiB free.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};

use std::thread;

use super::*;
use crate::loaders::LoaderPool;
use crate::testkit::{
    BOUND, Backend, Latch, MIB64, Scratch, Script, file_service, limits, state, status, step,
    teardown, wait_for_status, wait_until, wait_until_described, write_png,
};

const WIDTH: u32 = 4096;
const FILE_HEIGHT: u32 = 2048;
const MIB: u64 = 1024 * 1024;

struct Rig {
    service: ResourceService,
    script: Script,
    fake: Arc<Mutex<Backend>>,
    holders: Vec<TextureKey>,
    pool: Option<LoaderPool>,
    _scratch: Scratch,
}

impl Rig {
    /// A backend-mode service with `workers` loader threads (not started), three 32 MiB files `f1.png` to `f3.png`, a
    /// tiny `small.png`, and one holder per entry of `holders` (the height in rows of a 4096-wide image: 1024 rows are
    /// 16 MiB).
    fn new(workers: u32, holders: &[u32]) -> Self {
        let scratch = Scratch::new();
        for (index, colour) in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
            .into_iter()
            .enumerate()
        {
            write_png(
                scratch.path(),
                &format!("f{}.png", index + 1),
                WIDTH,
                FILE_HEIGHT,
                colour,
            );
        }
        write_png(scratch.path(), "small.png", 2, 2, [9, 9, 9, 255]);
        let service = file_service(scratch.path(), Mode::Backend, limits(workers, MIB64));
        let mut rig = Self {
            service,
            script: Script::default(),
            fake: Arc::new(Mutex::new(Backend::default())),
            holders: Vec::new(),
            pool: None,
            _scratch: scratch,
        };
        rig.script.install(&rig.service);
        for rows in holders {
            let key = rig.holder(*rows);
            rig.holders.push(key);
        }
        rig
    }

    /// A Rust resource of `rows` rows whose finished revision the fake backend keeps.
    fn holder(&self, rows: u32) -> TextureKey {
        let reservation = self.service.reserve_pixels(WIDTH, rows).expect("reserved");
        let bytes = vec![0; reservation.byte_len()];
        let pixels = CpuPixels::premultiplied(reservation, bytes).expect("the right length");
        let key = self.service.create_from_pixels(pixels).expect("admitted");
        step(&self.service).expect("the holder's job");
        self.backend().take_all(&self.service);
        key
    }

    fn backend(&self) -> MutexGuard<'_, Backend> {
        self.fake.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The fake backend drops the revision of `key`: its bytes return.
    fn release(&self, key: TextureKey) {
        assert_eq!(self.backend().release(key), 1, "the backend held it");
    }

    fn start(&mut self) {
        self.pool = Some(LoaderPool::start(self.service.clone()).expect("a pool"));
    }

    fn load(&self, name: &str) -> TextureKey {
        self.service.load(name).expect("admitted")
    }

    /// A tiny CPU resource, and waits until its job has run (a loader has evaluated the queues meanwhile).
    fn cpu_job_runs(&self) -> TextureKey {
        let key = self.cpu_job();
        wait_for_cpu_job_runs(&self.service, &self.script, key);
        key
    }

    fn cpu_job(&self) -> TextureKey {
        let reservation = self.service.reserve_pixels(2, 2).expect("reserved");
        let bytes = vec![255; reservation.byte_len()];
        let pixels = CpuPixels::straight_alpha(reservation, bytes).expect("the right length");
        self.service.create_from_pixels(pixels).expect("admitted")
    }

    fn deferred_keys(&self) -> Vec<TextureKey> {
        self.service.deferred().iter().map(|d| d.0).collect()
    }

    /// Loads `f1.png` to `f3.png`, starts the pool (three workers) and brings the three jobs to the deferred queue by
    /// the *refusals in the order 2, 3, 1* of the specification, each held before its reservation attempt until told.
    fn defer_three(&mut self) -> Vec<TextureKey> {
        let files: Vec<TextureKey> = (1..=3).map(|n| self.load(&format!("f{n}.png"))).collect();
        let holds: Vec<Latch> = files
            .iter()
            .map(|key| self.script.hold(Stage::BeforeAttempt, Some(*key)))
            .collect();
        self.start();
        for key in &files {
            self.script.wait_for(Stage::BeforeAttempt, Some(*key), 1);
        }
        for (order, index) in [1_usize, 2, 0].into_iter().enumerate() {
            holds[index].open();
            wait_until("the job joined the deferred queue", || {
                self.service.deferred().len() == order + 1
            });
        }
        files
    }

    fn finish(mut self) {
        self.script.open_all();
        if let Some(mut pool) = self.pool.take() {
            let report = pool.shutdown(BOUND);
            assert!(report.unfinished.is_empty(), "{report:?}");
        }
        self.backend().release_all();
        teardown(&self.service);
    }
}

impl Rig {
    /// After job 1 ran and jobs 2 and 3 stay deferred: the next two returns, each with the retry it triggers held at its
    /// attempt across the release (both capacity events of the release land while the retry is in flight), so the job after
    /// it is tried at once with the epoch final: job 2 runs and job 3 is tried at once and stays deferred; then job 3 runs.
    /// A late file job (`extra`) waits for admission to reopen and runs. Ends with every job at its one revision.
    fn release_the_rest(&mut self, files: &[TextureKey], extra: Option<TextureKey>) {
        // The next return: job 1's revision goes to the backend and is dropped.
        self.backend().take_all(&self.service);
        let tried = self.script.count(Stage::BeforeAttempt, Some(files[1]));
        let retry = self.script.hold(Stage::BeforeAttempt, Some(files[1]));
        self.release(files[0]);
        self.script
            .wait_for(Stage::BeforeAttempt, Some(files[1]), tried + 1);
        retry.open();
        wait_for_status(&self.service, files[1], "job 2 ran", |s| {
            s.state == StatusState::Uploading || s.state == StatusState::Ready
        });
        wait_until_described(
            "job 3 was tried at once and stays deferred",
            || {
                self.service
                    .deferred()
                    .iter()
                    .map(|d| d.0)
                    .collect::<Vec<_>>()
                    == vec![files[2]]
                    && self.script.count(Stage::BeforeAttempt, Some(files[2])) == 2
            },
            || describe_deferrals(self, files),
        );
        self.backend().take_all(&self.service);
        self.release(files[1]);
        wait_for_status(&self.service, files[2], "job 3 ran", |s| {
            s.state == StatusState::Uploading || s.state == StatusState::Ready
        });
        wait_until("file admission reopened", || {
            self.service.deferred().is_empty() && !self.service.retry_state().0
        });
        if let Some(extra) = extra {
            wait_for_status(&self.service, extra, "the late file job ran", |s| {
                s.state == StatusState::Uploading || s.state == StatusState::Ready
            });
        }
        for key in files {
            let now = status(&self.service, *key);
            assert_eq!(
                (now.requested, now.error),
                (1, None),
                "each ran to its one revision"
            );
        }
        assert!(
            !self.service.retry_state().1,
            "the deferral flag is clear when the queue is empty"
        );
    }
}

fn wait_for_cpu_job_runs(service: &ResourceService, script: &Script, key: TextureKey) {
    wait_for_status(service, key, "the CPU job ran", |s| {
        s.state == StatusState::Uploading
    });
    script.wait_for(Stage::Completed, Some(key), 1);
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.script.open_all();
    }
}

// ---------------------------------------------------------------------------------------------
// Concurrent deferrals (r3, F1)
// ---------------------------------------------------------------------------------------------

/// The state a failed wait of the concurrent test leaves, which decides between a test race (a count above what the test
/// expects, the queue in its expected order) and a defect in the deferral path (a job never re-examined, the queue wrong):
/// the deferred queue as (key, take sequence, recorded epoch), each file's `BeforeAttempt` count, the capacity epoch and
/// the retry state (a retry in flight, the deferral flag).
fn describe_deferrals(rig: &Rig, files: &[TextureKey]) -> String {
    let queue: Vec<_> = rig
        .service
        .deferred()
        .iter()
        .map(|d| (d.0, d.2, d.3))
        .collect();
    let counts: Vec<usize> = files
        .iter()
        .map(|key| rig.script.count(Stage::BeforeAttempt, Some(*key)))
        .collect();
    format!(
        "deferred (key, take sequence, recorded epoch) {queue:?}; jobs 1, 2, 3 are {files:?}; BeforeAttempt counts {counts:?}; capacity epoch {}; retry state (retrying, deferral flag) {:?}",
        rig.service.capacity_epoch(),
        rig.service.retry_state()
    )
}

/// Three file jobs taken (sequences 1, 2, 3) before any deferral exists are all refused, in the order 2, 3, 1: the
/// deferred queue holds 1, 2, 3 in that order with none lost or overwritten, no new file job is taken, a CPU job still
/// runs. Capacity returned once: job 1 alone is retried and, on success, job 2 is tried at once, then job 3; with only
/// enough bytes for one, job 1 runs and jobs 2 and 3 stay deferred until the next return; each job produces its
/// revision exactly once; every reservation is accounted at the end. Negative control: a single head slot loses job 2
/// or 3.
///
/// **One release is two capacity events**: dropping a revision first returns its bytes and then lowers its in-transit
/// count, and each is a capacity change that advances the epoch and wakes the loaders (`Capacity`'s documentation). If
/// the second event came after job 1 had succeeded and job 2 had been refused again, job 2 would be tried a third time,
/// correctly (`a_release_is_two_capacity_events_so_the_front_can_be_tried_a_third_time` forces that schedule). The test
/// therefore holds each retry at its attempt across the release that triggers it: both events land while that retry is
/// in flight (no second retry can start), and the job after it is then tried at once with the epoch already final, so
/// its count is exactly two on every schedule.
#[test]
fn concurrent_deferrals_queue_in_take_order_and_are_retried_one_at_a_time() {
    let mut rig = Rig::new(3, &[3072]);
    let holder = rig.holders[0];
    let files = rig.defer_three();
    let queued: Vec<(TextureKey, u64)> =
        rig.service.deferred().iter().map(|d| (d.0, d.2)).collect();
    assert_eq!(
        queued,
        vec![(files[0], 1), (files[1], 2), (files[2], 3)],
        "take order, none lost or overwritten"
    );
    for key in &files {
        assert_eq!(
            state(&rig.service, *key),
            StatusState::Pending,
            "deferred is pending"
        );
    }
    assert_eq!(
        rig.service.stats().cpu_pixels.reserved,
        48 * MIB,
        "a deferred job holds nothing image-sized"
    );

    // No new file job is taken while the queue is non-empty; a CPU job, admitted later, still runs.
    let extra = rig.load("small.png");
    rig.cpu_job_runs();
    assert_eq!(rig.script.count(Stage::BeforeAttempt, Some(extra)), 0);
    assert_eq!(state(&rig.service, extra), StatusState::Pending);

    // Capacity returned once: one release, two capacity events, both landing while job 1's retry is held at its attempt.
    let retry = rig.script.hold(Stage::BeforeAttempt, Some(files[0]));
    rig.release(holder);
    rig.script.wait_for(Stage::BeforeAttempt, Some(files[0]), 2);
    retry.open();
    wait_for_status(&rig.service, files[0], "job 1 ran", |s| {
        s.state == StatusState::Uploading
    });
    wait_until_described(
        "jobs 2 and 3 stay deferred",
        || {
            rig.service
                .deferred()
                .iter()
                .map(|d| (d.0, d.2))
                .collect::<Vec<_>>()
                == vec![(files[1], 2), (files[2], 3)]
                && rig.script.count(Stage::BeforeAttempt, Some(files[1])) == 2
        },
        || describe_deferrals(&rig, &files),
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[0])),
        2,
        "job 1: the refused attempt and its one retry"
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[1])),
        2,
        "job 2 was tried at once after job 1 succeeded, and refused again"
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[2])),
        1,
        "job 3 has not been tried: only the front is eligible"
    );
    rig.release_the_rest(&files, Some(extra));
    rig.finish();
}

/// Removes the service's transit hook on every path (a hook left in place would stall a later release).
struct TransitHook(ResourceService);

impl Drop for TransitHook {
    fn drop(&mut self) {
        self.0.set_transit_hook(None);
    }
}

/// Sets up the concurrent test's state: three file jobs deferred in the order 2, 3, 1 behind a 48 MiB holder, and a CPU job
/// (its 16 bytes are what leave a file job just short of the 32 MiB it needs).
fn three_deferred_and_a_cpu_job(rig: &mut Rig) -> Vec<TextureKey> {
    let files = rig.defer_three();
    rig.cpu_job_runs();
    files
}

/// The schedule that made the concurrent test's exact count unsatisfiable, forced. The release of the holder is two
/// capacity events (the return of its bytes, then the in-transit decrement); the transit hook holds the gap between them
/// until job 1 has succeeded, job 2 has been tried twice and the deferred queue is [job 2, job 3]. Then the decrement lands:
/// the epoch moves, the front (job 2) is eligible again and is tried a third time, correctly, and is deferred again in
/// front of job 3, which is still untried. (This is the state the old test's wait could never leave.)
#[test]
fn a_release_is_two_capacity_events_so_the_front_can_be_tried_a_third_time() {
    let mut rig = Rig::new(3, &[3072]);
    let holder = rig.holders[0];
    let files = three_deferred_and_a_cpu_job(&mut rig);
    let _hook = TransitHook(rig.service.clone());
    {
        let (service, script, keys) = (rig.service.clone(), rig.script.clone(), files.clone());
        rig.service.set_transit_hook(Some(Arc::new(move || {
            wait_until(
                "job 1 succeeded and job 2 was refused again, before the in-transit decrement",
                || {
                    state(&service, keys[0]) == StatusState::Uploading
                        && script.count(Stage::BeforeAttempt, Some(keys[1])) == 2
                        && service
                            .deferred()
                            .iter()
                            .map(|d| (d.0, d.2))
                            .collect::<Vec<_>>()
                            == vec![(keys[1], 2), (keys[2], 3)]
                },
            );
        })));
    }
    rig.release(holder);
    // Armed for this release only.
    rig.service.set_transit_hook(None);
    wait_until_described(
        "job 2 was tried a third time and deferred again in front of job 3",
        || {
            rig.script.count(Stage::BeforeAttempt, Some(files[1])) == 3
                && rig
                    .service
                    .deferred()
                    .iter()
                    .map(|d| (d.0, d.2))
                    .collect::<Vec<_>>()
                    == vec![(files[1], 2), (files[2], 3)]
                && !rig.service.retry_state().0
        },
        || describe_deferrals(&rig, &files),
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[2])),
        1,
        "job 3 is untried: only the front is eligible"
    );
    assert_eq!(
        state(&rig.service, files[1]),
        StatusState::Pending,
        "job 2 stays deferred"
    );
    rig.release_the_rest(&files, None);
    rig.finish();
}

/// The same hook with the fix's hold on job 1's retry: the hook waits only until job 1's retry is held at its attempt, so
/// the in-transit decrement lands while the retry is in flight (no second retry starts). After the hold is opened, job 1
/// succeeds and job 2 is tried at once with the epoch final: its count is exactly two and the queue is [job 2, job 3], on
/// this schedule as on every other.
#[test]
fn a_retry_held_across_the_release_makes_the_counts_independent_of_its_two_capacity_events() {
    let mut rig = Rig::new(3, &[3072]);
    let holder = rig.holders[0];
    let files = three_deferred_and_a_cpu_job(&mut rig);
    let _hook = TransitHook(rig.service.clone());
    {
        let (script, key) = (rig.script.clone(), files[0]);
        rig.service.set_transit_hook(Some(Arc::new(move || {
            script.wait_for(Stage::BeforeAttempt, Some(key), 2);
        })));
    }
    let retry = rig.script.hold(Stage::BeforeAttempt, Some(files[0]));
    rig.release(holder);
    rig.service.set_transit_hook(None);
    assert!(
        rig.service.retry_state().0,
        "the decrement landed while job 1's retry was in flight, and started no second one"
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[1])),
        1,
        "job 2 has not been tried again"
    );
    retry.open();
    wait_for_status(&rig.service, files[0], "job 1 ran", |s| {
        s.state == StatusState::Uploading
    });
    wait_until_described(
        "job 2 was tried at once and stays deferred",
        || {
            rig.service
                .deferred()
                .iter()
                .map(|d| (d.0, d.2))
                .collect::<Vec<_>>()
                == vec![(files[1], 2), (files[2], 3)]
                && rig.script.count(Stage::BeforeAttempt, Some(files[1])) == 2
        },
        || describe_deferrals(&rig, &files),
    );
    // A loader has evaluated the queues since (a CPU job ran): nothing made it a third time.
    rig.cpu_job_runs();
    assert_eq!(rig.script.count(Stage::BeforeAttempt, Some(files[1])), 2);
    assert_eq!(rig.script.count(Stage::BeforeAttempt, Some(files[2])), 1);
    assert_eq!(
        rig.service
            .deferred()
            .iter()
            .map(|d| (d.0, d.2))
            .collect::<Vec<_>>(),
        vec![(files[1], 2), (files[2], 3)]
    );
    rig.release_the_rest(&files, None);
    rig.finish();
}

/// During job 1's retry (held inside it): a capacity return does not start a second retry; a newer request for job 2's
/// resource replaces job 2 in place and waits in the ordinary queue; job 3's entry starting to retire drops job 3 at
/// `maintain`; and when the retry is let go, file admission reopens and the waiting request runs.
#[test]
fn during_a_retry_other_returns_do_not_start_another_and_requests_and_retirements_are_handled() {
    let mut rig = Rig::new(3, &[3072, 1024]);
    let (big, small) = (rig.holders[0], rig.holders[1]);
    let files = rig.defer_three();
    let jobs_before = rig.service.stats().jobs.reserved;
    let retry = rig.script.hold(Stage::BeforeAttempt, Some(files[0]));
    rig.release(big);
    rig.script.wait_for(Stage::BeforeAttempt, Some(files[0]), 2);
    assert!(rig.service.retry_state().0, "a retry is in flight");

    // (i) Another return: the epoch changes, no second retry starts.
    let epoch = rig.service.capacity_epoch();
    rig.release(small);
    assert!(rig.service.capacity_epoch() > epoch);
    rig.cpu_job_runs();
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[1])),
        1,
        "job 2 was not retried during job 1's retry"
    );

    // (ii) A newer request for job 2's resource replaces it in place: its job slot passes on.
    assert_eq!(rig.service.reload(files[1]), Ok(2));
    assert_eq!(
        rig.deferred_keys(),
        vec![files[2]],
        "job 2 left the deferred queue"
    );
    assert_eq!(state(&rig.service, files[1]), StatusState::Pending);
    assert_eq!(
        rig.service.stats().jobs.reserved,
        jobs_before,
        "the reload took over job 2's slot: no new one"
    );

    // (iii) Job 3's resource starts to retire: `maintain` (a loader runs it) drops its deferred job.
    rig.service.release(files[2]).expect("released");
    wait_until("job 3 dropped from the deferred queue", || {
        rig.service.deferred().is_empty()
    });
    assert!(
        rig.service.retry_state().0,
        "the retry is still in flight: admission stays closed"
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(files[1])),
        1,
        "and the waiting request has not been taken"
    );

    // The retry is let go: it reserves, runs, and file admission reopens for the waiting request.
    retry.open();
    wait_for_status(&rig.service, files[0], "job 1 ran", |s| {
        s.state == StatusState::Uploading
    });
    // File admission reopened: the waiting request is taken; only a few bytes short, it waits for job 1's revision to
    // return, in its turn.
    wait_until("the replacing request was tried", || {
        rig.script.count(Stage::BeforeAttempt, Some(files[1])) == 2
    });
    assert!(!rig.service.retry_state().0);
    rig.backend().take_all(&rig.service);
    rig.release(files[0]);
    wait_for_status(&rig.service, files[1], "the replacing request ran", |s| {
        s.state == StatusState::Uploading && s.requested == 2
    });
    rig.finish();
}

/// A CPU job's status becomes Uploading before its job slot is returned. The helper waits for Completed, after the
/// ticket drops, so callers that inspect job counts cannot observe the publication window.
#[test]
fn cpu_job_runs_waits_for_the_published_job_to_complete() {
    let mut rig = Rig::new(1, &[]);
    let jobs_before = rig.service.stats().jobs.reserved;
    let key = rig.cpu_job();
    let published = rig.script.hold(Stage::Published, Some(key));
    rig.start();

    rig.script.wait_for(Stage::Published, Some(key), 1);
    assert_eq!(state(&rig.service, key), StatusState::Uploading);
    assert_eq!(rig.service.stats().jobs.reserved, jobs_before + 1);
    assert_eq!(rig.script.count(Stage::Completed, Some(key)), 0);

    let service = rig.service.clone();
    let script = rig.script.clone();
    let (returned_tx, returned_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        wait_for_cpu_job_runs(&service, &script, key);
        returned_tx.send(()).expect("the CPU job helper returned");
    });

    assert!(
        returned_rx.try_recv().is_err(),
        "the CPU job helper must wait while Published is held"
    );
    assert_eq!(rig.script.count(Stage::Completed, Some(key)), 0);

    published.open();
    rig.script.wait_for(Stage::Completed, Some(key), 1);
    assert_eq!(rig.service.stats().jobs.reserved, jobs_before);
    returned_rx
        .recv_timeout(BOUND)
        .expect("the CPU job helper returns after Completed");
    waiter
        .join()
        .expect("the CPU job helper thread did not panic");
    rig.finish();
}

/// A retry whose resource starts to retire while it runs has its result discarded at `complete`, the retry mark clears,
/// and the next front is eligible at once (without any return of bytes).
#[test]
fn a_retry_discarded_because_its_resource_retires_hands_the_next_front_its_turn() {
    let mut rig = Rig::new(3, &[3072]);
    let holder = rig.holders[0];
    let files = rig.defer_three();
    let retry = rig.script.hold(Stage::BeforeAttempt, Some(files[0]));
    // The holder's bytes return: job 1's retry starts and is held before its attempt.
    rig.release(holder);
    rig.script.wait_for(Stage::BeforeAttempt, Some(files[0]), 2);
    // Job 1's resource starts to retire (a loader's `maintain` marks it discard-only) while its retry executes.
    rig.service.release(files[0]).expect("released");
    wait_until("job 1's retiring entry was seen", || {
        rig.service.stats().registry.retiring == 1 && !rig.service.retirement_pending()
    });
    let attempts_of_2 = rig.script.count(Stage::BeforeAttempt, Some(files[1]));
    retry.open();
    // Job 1's retry reserves, decodes and is discarded; the mark clears and job 2 is eligible at once.
    rig.script
        .wait_for(Stage::BeforeAttempt, Some(files[1]), attempts_of_2 + 1);
    wait_for_status(&rig.service, files[1], "job 2 ran", |s| {
        s.state == StatusState::Uploading
    });
    rig.finish();
}

// ---------------------------------------------------------------------------------------------
// The handshake
// ---------------------------------------------------------------------------------------------

/// With the front deferred and bytes in transit, queuing CPU jobs and making spurious wakes keeps its reservation
/// attempts at one; later file jobs are not taken while the queue is non-empty, CPU jobs are. Then, with no call by the
/// test thread but the backend dropping its `Arc`, the front is retried and completes. Negative control: without the
/// wake on returned bytes the deferred job never resumes and the bounded wait fails.
#[test]
fn a_deferred_front_is_not_retried_without_a_return_and_resumes_after_one_with_no_call() {
    let mut rig = Rig::new(2, &[3072]);
    let holder = rig.holders[0];
    let file = rig.load("f1.png");
    rig.start();
    wait_until("the job is deferred", || rig.deferred_keys() == vec![file]);
    assert_eq!(rig.script.count(Stage::BeforeAttempt, Some(file)), 1);
    let later = rig.load("f2.png");
    for _ in 0..5 {
        rig.cpu_job_runs();
        rig.service.poke();
    }
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(file)),
        1,
        "no repeated retry without a relevant change"
    );
    assert_eq!(
        rig.script.count(Stage::BeforeAttempt, Some(later)),
        0,
        "later file jobs wait"
    );
    assert_eq!(state(&rig.service, later), StatusState::Pending);
    // The backend drops its Arc; nothing else is called.
    rig.release(holder);
    wait_for_status(&rig.service, file, "the deferred job resumed", |s| {
        s.state == StatusState::Uploading
    });
    // File admission reopened: the later job is tried, and waits for the next return in its turn.
    wait_until("the later job was deferred", || {
        rig.deferred_keys() == vec![later]
    });
    rig.backend().take_all(&rig.service);
    rig.release(file);
    wait_for_status(&rig.service, later, "the later job ran", |s| {
        s.state == StatusState::Uploading
    });
    rig.finish();
}

/// A return **before** the flag is set (between the refused attempt and the flag), **between** setting it and recording
/// the deferral, and **after** recording it: in each the front becomes eligible (immediately or by the notification),
/// never sleeping forever. Negative controls: a deferral recorded with the epoch read at recording time loses the first
/// two; a returner that does not notify loses the third.
#[test]
fn a_return_at_any_point_of_the_handshake_is_not_lost() {
    for stage in [Stage::Refused, Stage::Checked, Stage::FlagRaised] {
        let mut rig = Rig::new(1, &[3072]);
        let holder = rig.holders[0];
        let file = rig.load("f1.png");
        let fake = Arc::clone(&rig.fake);
        rig.script.on(stage, Some(file), move |_| {
            // The first time only: the retry reaches this point again only if it is refused again.
            let _ = fake
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .release(holder);
        });
        rig.start();
        wait_for_status(
            &rig.service,
            file,
            &format!("a return at {stage:?} is not lost"),
            |s| s.state == StatusState::Uploading,
        );
        rig.finish();
    }

    // After recording: the loader that recorded it is held; the other one sleeps; the return is made by the test thread
    // and must wake the sleeper.
    let mut rig = Rig::new(2, &[3072]);
    let holder = rig.holders[0];
    let file = rig.load("f1.png");
    let held = rig.script.hold(Stage::Recorded, Some(file));
    rig.start();
    rig.script.wait_for(Stage::Recorded, Some(file), 1);
    wait_until("the other loader is asleep", || {
        rig.service.waiting_loaders() == 1
    });
    assert_eq!(rig.deferred_keys(), vec![file]);
    rig.release(holder);
    wait_for_status(
        &rig.service,
        file,
        "the sleeper was woken by the return",
        |s| s.state == StatusState::Uploading,
    );
    held.open();
    rig.finish();
}

/// A refusal with nothing in transit (every byte held by a live caller reservation) fails the request with the capacity
/// error at once, in both modes; nothing blocks.
#[test]
fn a_refusal_with_nothing_in_transit_is_a_capacity_error() {
    for mode in [Mode::Backend, Mode::Headless] {
        let scratch = Scratch::new();
        write_png(scratch.path(), "f1.png", WIDTH, FILE_HEIGHT, [1, 2, 3, 255]);
        let service = file_service(scratch.path(), mode, limits(1, MIB64));
        // 48 MiB held by a caller's reservation: not finished pixels, so nothing is in transit.
        let held = service.reserve_pixels(WIDTH, 3072).expect("reserved");
        let key = service.load("f1.png").expect("admitted");
        assert_eq!(
            step(&service),
            Some((key, 1)),
            "the job returns: it does not wait"
        );
        let now = status(&service, key);
        assert_eq!(now.state, StatusState::Failed);
        let message = now.error.expect("an error").message().to_owned();
        assert!(
            message.contains("not enough CPU pixel bytes")
                && message.contains(&format!("{} needed", 32 * MIB)),
            "{message}"
        );
        assert!(service.deferred().is_empty());
        drop(held);
        teardown(&service);
    }
}

/// A refusal while finished pixels are in transit is not a failure: the job is deferred (`pending`, no image-sized
/// allocation held) and, once the fake backend drops its `Arc`, it is handed out again as the retry and completes.
#[test]
fn a_refusal_while_pixels_are_in_transit_waits_and_is_retried_once_they_return() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "f1.png", WIDTH, FILE_HEIGHT, [1, 2, 3, 255]);
    let service = file_service(scratch.path(), Mode::Backend, limits(1, MIB64));
    let reservation = service.reserve_pixels(WIDTH, 3072).expect("reserved");
    let bytes = vec![0; reservation.byte_len()];
    let cpu = service
        .create_from_pixels(CpuPixels::premultiplied(reservation, bytes).expect("length"))
        .expect("admitted");
    step(&service).expect("the holder's job");
    let mut fake = Backend::default();
    fake.take_all(&service);

    let key = service.load("f1.png").expect("admitted");
    let first = service.take_job().expect("the file job");
    let output = first.run();
    assert!(output.is_deferred());
    service.complete(output);
    assert_eq!(state(&service, key), StatusState::Pending);
    assert_eq!(
        service.stats().cpu_pixels.reserved,
        48 * MIB,
        "nothing image-sized is held"
    );
    assert_eq!(service.deferred().len(), 1);
    assert!(service.take_job().is_none(), "no return yet: no retry");
    assert!(fake.release(cpu) == 1);
    let retry = service.take_job().expect("the retry, after the return");
    assert_eq!(
        state(&service, key),
        StatusState::Decoding,
        "a retry counts as executing"
    );
    service.complete(retry.run());
    assert_eq!(state(&service, key), StatusState::Uploading);
    assert_eq!(status(&service, key).error, None);
    fake.take_all(&service);
    fake.release_all();
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Deferral and retirement (r2)
// ---------------------------------------------------------------------------------------------

/// The deferral is an acceptance decision. (a) The resource's last reference is dropped before the decision and
/// `maintain` has not run: the job is discarded, its job slot returns, the retirement finishes once `maintain` runs.
/// (b) The last reference is dropped after the job joined the deferred queue: `maintain` drops the deferred job and the
/// retirement finishes. In neither case is the job taken again. Negative control: re-queuing without the lifecycle
/// check leaves (a) with a queued job of a retiring entry.
#[test]
fn a_deferral_is_discarded_or_dropped_when_the_resource_retires() {
    for before_the_decision in [true, false] {
        let scratch = Scratch::new();
        write_png(scratch.path(), "f1.png", WIDTH, FILE_HEIGHT, [1, 2, 3, 255]);
        let service = file_service(scratch.path(), Mode::Backend, limits(1, MIB64));
        let reservation = service.reserve_pixels(WIDTH, 3072).expect("reserved");
        let bytes = vec![0; reservation.byte_len()];
        let holder = service
            .create_from_pixels(CpuPixels::premultiplied(reservation, bytes).expect("length"))
            .expect("admitted");
        step(&service).expect("the holder's job");
        let mut fake = Backend::default();
        fake.take_all(&service);

        let key = service.load("f1.png").expect("admitted");
        let reference = service.retain(key).expect("a reference");
        service.release(key).expect("public ownership ends");
        let job = service.take_job().expect("the file job");
        let output = job.run();
        assert!(output.is_deferred());
        if before_the_decision {
            drop(reference);
            service.complete(output);
            assert!(
                service.deferred().is_empty(),
                "(a) the job was discarded, not kept"
            );
            assert_eq!(
                service.stats().jobs.reserved,
                0,
                "(a) its job slot returned"
            );
            assert!(service.queued_keys().0.is_empty());
        } else {
            service.complete(output);
            assert_eq!(service.deferred().len(), 1, "(b) kept");
            drop(reference);
        }
        // Cleaned up by `maintain`, which finishes the retirement.
        fake.release(holder);
        let started = service.stats().registry.retiring;
        assert_eq!(started, 1);
        let mut keys = Vec::new();
        for _ in 0..4 {
            let _ = service.maintain(8);
            service.take_retirements(&mut keys, 8);
            for retired in keys.drain(..) {
                service.retired(retired);
            }
        }
        assert!(service.deferred().is_empty());
        assert!(service.take_job().is_none(), "the job is not taken again");
        assert_eq!(
            service.stats().registry.retiring,
            0,
            "the retirement finished"
        );
        fake.release_all();
        teardown(&service);
    }
}

/// Once the bytes of every revision were returned and dropped, nothing is in transit: a refusal is a capacity error at
/// once, not a wait for bytes that will never come. (Negative control: in-transit bytes never counted down.)
#[test]
fn a_refusal_after_every_revision_was_dropped_is_a_capacity_error_not_a_wait() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "f1.png", WIDTH, FILE_HEIGHT, [1, 2, 3, 255]);
    let service = file_service(scratch.path(), Mode::Backend, limits(1, MIB64));
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bytes = vec![0; reservation.byte_len()];
    let tiny = service
        .create_from_pixels(CpuPixels::premultiplied(reservation, bytes).expect("length"))
        .expect("admitted");
    step(&service).expect("the tiny job");
    let mut fake = Backend::default();
    fake.take_all(&service);
    fake.release(tiny);
    let held = service
        .reserve_pixels(WIDTH, 3072)
        .expect("a live caller reservation");
    let key = service.load("f1.png").expect("admitted");
    assert_eq!(step(&service), Some((key, 1)));
    let now = status(&service, key);
    assert_eq!(now.state, StatusState::Failed, "{now:?}");
    assert!(
        now.error
            .expect("an error")
            .message()
            .contains("not enough CPU pixel bytes")
    );
    assert!(service.deferred().is_empty());
    drop(held);
    teardown(&service);
}

/// A retry's reservation succeeding decides its outcome: file admission may reopen and the next deferred job is eligible
/// **at once** for one attempt, even though no bytes have returned since its own refused attempt (its epoch is the
/// current one). Tested on the state, because a schedule that needs it requires a deferral recorded after the last
/// return and before the previous retry's reservation, which the real protocol only produces in narrow races.
#[test]
fn after_a_retry_reserves_the_next_front_is_eligible_at_once_whatever_its_epoch() {
    let scratch = Scratch::new();
    let service = file_service(scratch.path(), Mode::Backend, limits(1, MIB64));
    let epoch = service.capacity_epoch();
    let key = |n: u64| TextureKey::new(service.registry().id(), u32::try_from(n).expect("slot"), 1);
    service.plant_deferral(key(2), 1, 2, epoch);
    service.plant_deferral(key(3), 1, 3, epoch);
    assert!(!service.front_eligible_now(), "same epoch: not eligible");
    service.plant_retry(key(1), 1);
    service.retry_reserved(key(1), 1);
    assert!(
        service.front_eligible_now(),
        "eligible at once after the retry's reservation"
    );
    // A reservation reported for a job that is not the retry changes nothing.
    service.plant_retry(key(9), 1);
    service.retry_reserved(key(9), 2);
    assert!(
        service.retry_state().0,
        "the mark of another revision stands"
    );
}

/// A refusal in the gap between a revision's return of bytes and the decrement of its in-transit count: the bytes are
/// back (the epoch advanced before the loader reads it) but still counted in transit, so the refusal looks deferrable and
/// is recorded with an epoch that no return will ever change. The decrement is itself a capacity change, so the front
/// becomes eligible and the retry settles it: nothing is in transit, and the rest of the budget belongs to a live caller
/// reservation, so it is the capacity error. (Negative control: a decrement that advances no epoch strands the job.)
#[test]
fn a_refusal_between_the_return_and_the_last_transit_decrement_is_settled_by_the_decrement() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "f1.png", WIDTH, FILE_HEIGHT, [1, 2, 3, 255]);
    let service = file_service(scratch.path(), Mode::Backend, limits(1, MIB64));
    // 16 MiB held by a backend revision (in transit), 48 MiB by a live caller reservation.
    let reservation = service.reserve_pixels(WIDTH, 1024).expect("reserved");
    let bytes = vec![0; reservation.byte_len()];
    let holder = service
        .create_from_pixels(CpuPixels::premultiplied(reservation, bytes).expect("length"))
        .expect("admitted");
    step(&service).expect("the holder's job");
    let mut fake = Backend::default();
    fake.take_all(&service);
    let live = service
        .reserve_pixels(WIDTH, 3072)
        .expect("a live caller reservation");
    // The last revision's drop is paused after its bytes were returned, before its in-transit count goes down.
    let (reached, release) = (Latch::default(), Latch::default());
    {
        let (reached, release) = (reached.clone(), release.clone());
        service.set_transit_hook(Some(Arc::new(move || {
            reached.open();
            release.wait();
        })));
    }
    let dropper = thread::spawn(move || {
        fake.release(holder);
        fake
    });
    reached.wait();
    let key = service.load("f1.png").expect("admitted");
    let output = service.take_job().expect("the file job").run();
    assert!(
        output.is_deferred(),
        "the refusal looks deferrable: 16 MiB are still counted in transit"
    );
    service.complete(output);
    assert_eq!(service.deferred().len(), 1);
    assert!(service.take_job().is_none(), "nothing has changed yet");
    release.open();
    let mut fake = dropper.join().expect("the drop finished");
    let retry = service
        .take_job()
        .expect("the decrement itself made the front eligible");
    service.complete(retry.run());
    let now = status(&service, key);
    assert_eq!(now.state, StatusState::Failed, "{now:?}");
    assert!(
        now.error
            .expect("an error")
            .message()
            .contains("not enough CPU pixel bytes"),
        "the capacity error, without another return or a wake"
    );
    assert!(service.deferred().is_empty());
    drop(live);
    fake.release_all();
    teardown(&service);
}

//! CK-108: the loader pool, with real threads. No test sleeps for timing: it waits on observable state with a bounded
//! deadline and holds threads with latches at the points of the protocols. Each ends by closing the service, retiring
//! everything and asserting all three budgets are back to zero and that no value was dropped with a lock held.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use super::*;
use crate::pixels::CpuPixels;
use crate::registry::TextureKey;
use crate::service::AdmitError;
use crate::service::{Mode, ResourceService, ServiceWakes, Stage, StatusState};
use crate::testkit::{
    BOUND, Backend, Latch, MIB64, Scratch, Script, file_service, limits, reserved, state, status,
    teardown, wait_for_status, wait_until, write_png,
};
use crate::unwind::{Bomb, NON_STRING_PAYLOAD};

/// A headless service with `workers` loader threads' worth of limits and a `small.png` below its root.
fn rig(workers: u32, mode: Mode) -> (Scratch, ResourceService) {
    let scratch = Scratch::new();
    write_png(scratch.path(), "small.png", 2, 2, [7, 7, 7, 255]);
    let service = file_service(scratch.path(), mode, limits(workers, MIB64));
    (scratch, service)
}

fn cpu(service: &ResourceService) -> TextureKey {
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bytes = vec![255; reservation.byte_len()];
    let pixels = CpuPixels::straight_alpha(reservation, bytes).expect("the right length");
    service.create_from_pixels(pixels).expect("admitted")
}

fn replacement(service: &ResourceService) -> CpuPixels {
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bytes = vec![255; reservation.byte_len()];
    CpuPixels::straight_alpha(reservation, bytes).expect("the right length")
}

fn ready(state: StatusState) -> bool {
    matches!(state, StatusState::CpuReady | StatusState::Ready)
}

/// Closes the pool (everything finished in time) and the service's books.
fn end(mut pool: LoaderPool, service: &ResourceService, script: &Script) {
    script.open_all();
    let report = pool.shutdown(BOUND);
    assert!(report.unfinished.is_empty(), "{report:?}");
    assert!(report.loop_failures.is_empty(), "{report:?}");
    let _ = service.finish_teardown();
    teardown(service);
}

// ---------------------------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------------------------

/// With N workers at most N jobs run at once (held at their start), and never two for one resource: a replacement
/// admitted while a job of the resource is held waits for it.
#[test]
fn at_most_n_jobs_run_at_once_and_never_two_for_one_resource() {
    const WORKERS: u32 = 3;
    let (_scratch, service) = rig(WORKERS, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let running: Arc<Mutex<Vec<TextureKey>>> = Arc::default();
    let doubled = Arc::new(AtomicBool::new(false));
    {
        let (active, peak, running, doubled) = (
            Arc::clone(&active),
            Arc::clone(&peak),
            Arc::clone(&running),
            Arc::clone(&doubled),
        );
        script.on(Stage::Run, None, move |key| {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            let key = key.expect("a job has a key");
            let mut running = running.lock().unwrap_or_else(PoisonError::into_inner);
            if running.contains(&key) {
                doubled.store(true, Ordering::SeqCst);
            }
            running.push(key);
        });
    }
    {
        let (active, running) = (Arc::clone(&active), Arc::clone(&running));
        script.on(Stage::Ran, None, move |key| {
            active.fetch_sub(1, Ordering::SeqCst);
            running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|running| Some(*running) != key);
        });
    }
    let gate = script.hold(Stage::Run, None);
    let keys: Vec<TextureKey> = (0..8).map(|_| cpu(&service)).collect();
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, None, WORKERS as usize);
    // A replacement for a resource whose job is held: it queues behind it.
    assert_eq!(
        service.replace_pixels(keys[0], replacement(&service)).ok(),
        Some(2)
    );
    gate.open();
    for key in &keys {
        wait_for_status(&service, *key, "every job ran", |s| ready(s.state));
    }
    wait_for_status(&service, keys[0], "the replacement ran", |s| {
        s.shown.map(|shown| shown.revision) == Some(2)
    });
    assert_eq!(
        peak.load(Ordering::SeqCst),
        WORKERS as usize,
        "at most N at once, and N were"
    );
    assert!(
        !doubled.load(Ordering::SeqCst),
        "two jobs ran for one resource"
    );
    end(pool, &service, &script);
}

/// Jobs queued while every worker sleeps are picked up: no lost wake, over many rounds each with a bounded wait.
#[test]
fn a_job_queued_while_every_worker_sleeps_is_picked_up() {
    const WORKERS: u32 = 2;
    let (_scratch, service) = rig(WORKERS, Mode::Headless);
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    for round in 0..150 {
        wait_until("every worker asleep", || {
            service.waiting_loaders() == WORKERS as usize
        });
        let key = if round % 2 == 0 {
            cpu(&service)
        } else {
            service.load("small.png").expect("admitted")
        };
        wait_for_status(&service, key, "the sleeping pool picked the job up", |s| {
            ready(s.state)
        });
        service.release(key).expect("released");
    }
    end(pool, &service, &Script::default());
}

/// Dropping the last `TextureRef` on the test thread, with no service call, gets the resource retired by the pool: the
/// registry's retiring wake reaches a sleeping loader.
#[test]
fn dropping_the_last_reference_gets_the_resource_retired_by_the_pool() {
    let (_scratch, service) = rig(2, Mode::Headless);
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    let key = cpu(&service);
    let reference = service.retain(key).expect("a reference");
    wait_for_status(&service, key, "the job ran", |s| ready(s.state));
    service.release(key).expect("public ownership ends");
    wait_until("every worker asleep", || service.waiting_loaders() == 2);
    drop(reference);
    wait_until("the pool retired it", || {
        let registry = service.stats().registry;
        registry.live == 0 && registry.retiring == 0
    });
    end(pool, &service, &Script::default());
}

/// Headless (§8.3): the same pool runs CPU and file jobs, statuses reach `cpu_ready` or `failed`, and the run ends with
/// `shutdown` then `finish_teardown`.
#[test]
fn a_headless_pool_runs_cpu_and_file_jobs_to_cpu_ready_or_failed_and_ends_finitely() {
    let (scratch, service) = rig(2, Mode::Headless);
    std::fs::write(scratch.path().join("garbage.png"), b"not a png").expect("a file");
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    let good = service.load("small.png").expect("admitted");
    let bad = service.load("garbage.png").expect("admitted");
    let missing = service.load("missing.png").expect("admitted");
    let from_cpu = cpu(&service);
    wait_for_status(&service, good, "the good file", |s| {
        s.state == StatusState::CpuReady
    });
    wait_for_status(&service, from_cpu, "the CPU source", |s| {
        s.state == StatusState::CpuReady
    });
    for key in [bad, missing] {
        let failed = wait_for_status(&service, key, "the bad file", |s| {
            s.state == StatusState::Failed
        });
        assert!(failed.error.is_some());
    }
    end(pool, &service, &Script::default());
}

// ---------------------------------------------------------------------------------------------
// Panics in a job
// ---------------------------------------------------------------------------------------------

/// A job made to panic, with an ordinary payload, with a payload whose `Drop` panics, and (for a file) after its
/// reservation was granted, fails its request with the message, returns its reservations, and the thread runs the next
/// job. Negative control: without the per-job `catch_unwind` the thread ends and the next job is never run.
#[test]
fn a_panicking_job_fails_its_request_returns_its_reservations_and_the_thread_runs_the_next() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let ordinary = cpu(&service);
    let bomb = cpu(&service);
    let file = service.load("small.png").expect("admitted");
    let next = cpu(&service);
    let drops = Arc::new(AtomicUsize::new(0));
    let dropped_on = Arc::new(Mutex::new(Vec::new()));
    script.on(Stage::Run, Some(ordinary), |_| panic!("boom"));
    {
        let (drops, dropped_on) = (Arc::clone(&drops), Arc::clone(&dropped_on));
        script.on(Stage::Run, Some(bomb), move |_| {
            std::panic::panic_any(Bomb::new(&drops, &dropped_on));
        });
    }
    script.on(Stage::Reserved, Some(file), |_| {
        panic!("after the reservation")
    });
    let pool = LoaderPool::start(service.clone()).expect("a pool");

    let failed = wait_for_status(
        &service,
        ordinary,
        "the ordinary panic failed its request",
        |s| s.state == StatusState::Failed,
    );
    assert_eq!(
        failed.error.expect("an error").message(),
        "the loader panicked: boom"
    );
    let failed = wait_for_status(&service, bomb, "the bomb failed its request", |s| {
        s.state == StatusState::Failed
    });
    assert_eq!(
        failed.error.expect("an error").message(),
        format!("the loader panicked: {NON_STRING_PAYLOAD}")
    );
    let failed = wait_for_status(&service, file, "the file job failed its request", |s| {
        s.state == StatusState::Failed
    });
    assert_eq!(
        failed.error.expect("an error").message(),
        "the loader panicked: after the reservation"
    );
    wait_for_status(&service, next, "the thread ran the next job", |s| {
        ready(s.state)
    });
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the payload was dropped, once"
    );
    let ran_on = dropped_on
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(ran_on.len(), 1);
    assert_ne!(ran_on[0], thread::current().id(), "on the loader thread");
    // Readiness is published before the completion drops its discards and its ticket: the returns are waited for.
    wait_until_job_budgets_returned(&service);
    let stats = service.stats();
    assert_eq!(stats.loaders.job_panics, 3);
    assert_eq!(stats.jobs.reserved, 0, "every job slot returned");
    assert_eq!(
        stats.cpu_pixels.reserved, 0,
        "every CPU reservation returned"
    );
    end(pool, &service, &script);
}

// ---------------------------------------------------------------------------------------------
// Teardown ownership
// ---------------------------------------------------------------------------------------------

/// The three budgets, checked separately.
fn assert_zero_separately(service: &ResourceService) {
    let stats = service.stats();
    assert_eq!(stats.jobs.reserved, 0, "job slots");
    assert_eq!(stats.cpu_pixels.reserved, 0, "CPU bytes");
    assert_eq!(stats.metadata.reserved, 0, "metadata bytes");
}

/// (a) The last loader is held in a job of a resource whose last reference is dropped; shutdown detaches it; its late
/// completion finishes the retirement inline.
#[test]
fn a_detached_loader_finishes_the_retirement_of_its_resource_at_its_late_completion() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = service.load("small.png").expect("admitted");
    let held = script.hold(Stage::Run, Some(key));
    let reference = service.retain(key).expect("a reference");
    service.release(key).expect("public ownership ends");
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, Some(key), 1);
    drop(reference);
    let report = pool.shutdown(Duration::ZERO);
    assert_eq!(
        report.unfinished,
        vec!["ck-loader-0".to_owned()],
        "{report:?}"
    );
    // The loader is detached: nobody else drains. The teardown drains the key; the executing job blocks it.
    assert_eq!(service.finish_teardown().finished, 0);
    assert_eq!(service.stats().registry.retiring, 1);
    held.open();
    wait_until("the late completion finished it inline", || {
        service.stats().loaders.inline_finishes == 1
    });
    // The counter is published before the completion has dropped the state and returned the job's slot, metadata and
    // bytes (see the control below): the returned budgets are waited for, never assumed from the counter.
    wait_until_returned(&service);
    assert_eq!(service.stats().registry.retiring, 0);
    assert_zero_separately(&service);
    teardown(&service);
}

/// Waits (bounded) until the job slots, the metadata and the CPU bytes are all back, and the registry has nothing
/// retiring: what the late completion's cleanup establishes, after the counter it publishes first.
fn wait_until_returned(service: &ResourceService) {
    wait_until("the late completion returned its budgets", || {
        let stats = service.stats();
        stats.jobs.reserved == 0
            && stats.cpu_pixels.reserved == 0
            && stats.metadata.reserved == 0
            && stats.registry.retiring == 0
    });
}

/// Waits (bounded) until every job slot and CPU reservation is back: a request that reads as ready or failed has
/// published its result, but its completion drops the pixel discards and its ticket afterwards.
fn wait_until_job_budgets_returned(service: &ResourceService) {
    wait_until("the completions returned their job slots and bytes", || {
        let stats = service.stats();
        stats.jobs.reserved == 0 && stats.cpu_pixels.reserved == 0
    });
}

/// Waits (bounded) until the job slots, the CPU bytes and the metadata charges are all back, after the teardown has
/// returned what the registry held: what is left is a detached completion's ticket, whose slot (the first field) goes
/// back before its metadata charge (the second), so a slot count of zero is not the end of its cleanup.
fn wait_until_budgets_zero(service: &ResourceService) {
    wait_until("the detached completion returned all its budgets", || {
        let stats = service.stats();
        stats.jobs.reserved == 0 && stats.cpu_pixels.reserved == 0 && stats.metadata.reserved == 0
    });
}

/// The control for readiness before cleanup: with the completion held after it published the result, the request
/// already reads as ready, while its job slot is still charged - so readiness is not a join. Released, the completion
/// finishes and the slot is back.
#[test]
fn a_ready_request_still_holds_its_job_slot_until_its_completion_has_cleaned_up() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = cpu(&service);
    let published = script.hold(Stage::Published, Some(key));
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    wait_for_status(&service, key, "the request read as ready", |s| {
        ready(s.state)
    });
    assert_eq!(
        service.stats().jobs.reserved,
        1,
        "ready, and the slot is not yet back: readiness is no join"
    );
    published.open();
    script.wait_for(Stage::Completed, Some(key), 1);
    wait_until_job_budgets_returned(&service);
    assert_eq!(service.stats().jobs.reserved, 0, "job slots");
    end(pool, &service, &script);
}

/// The control for the order of a ticket's returns: with the completion held between its slot's return and its
/// metadata charge's, the job slots read zero while the charge is still counted - so a slot count of zero is not the
/// end of the cleanup. Released, exactly one job's metadata charge goes back.
#[test]
fn a_returned_job_slot_does_not_mean_the_metadata_charge_is_back() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = cpu(&service);
    let between = script.hold(Stage::SlotReturned, None);
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::SlotReturned, None, 1);
    let held = service.stats();
    assert_eq!(held.jobs.reserved, 0, "the slot is back");
    assert!(
        held.metadata.reserved >= crate::service::JOB_METADATA_BYTES,
        "and the job's metadata charge is still counted: {held:?}"
    );
    between.open();
    script.wait_for(Stage::Completed, Some(key), 1);
    wait_until_job_budgets_returned(&service);
    let after = service.stats();
    assert_eq!(
        held.metadata.reserved - after.metadata.reserved,
        crate::service::JOB_METADATA_BYTES,
        "exactly the job's charge went back: {held:?} then {after:?}"
    );
    end(pool, &service, &script);
}

/// The control for the early counter: with the completing loader held between the inline finish and the cleanup, the
/// counter already says one, while the job's slot is still reserved - so the counter is not a join. Released, the
/// budgets reach zero.
#[test]
fn the_inline_finish_counter_is_visible_before_the_late_completion_has_returned_its_budgets() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = service.load("small.png").expect("admitted");
    let run = script.hold(Stage::Run, Some(key));
    let finished = script.hold(Stage::Finished, Some(key));
    let reference = service.retain(key).expect("a reference");
    service.release(key).expect("public ownership ends");
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, Some(key), 1);
    drop(reference);
    let report = pool.shutdown(Duration::ZERO);
    assert_eq!(report.unfinished.len(), 1, "{report:?}");
    assert_eq!(service.finish_teardown().finished, 0);
    run.open();
    script.wait_for(Stage::Finished, Some(key), 1);
    let stats = service.stats();
    assert_eq!(
        stats.loaders.inline_finishes, 1,
        "the counter is already published"
    );
    assert_eq!(
        stats.jobs.reserved, 1,
        "the job's slot is not yet back: the counter is no join"
    );
    finished.open();
    wait_until_returned(&service);
    assert_zero_separately(&service);
    teardown(&service);
}

/// (b) The last `TextureRef` is dropped after every loader exited and before `finish_teardown`, which then finishes it.
#[test]
fn a_reference_dropped_after_the_pool_ended_is_finished_by_the_teardown() {
    let (_scratch, service) = rig(2, Mode::Headless);
    let key = service.load("small.png").expect("admitted");
    let reference = service.retain(key).expect("a reference");
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    wait_for_status(&service, key, "the job ran", |s| ready(s.state));
    let report = pool.shutdown(BOUND);
    assert_eq!(report.finished.len(), 2, "{report:?}");
    drop(reference);
    assert_eq!(
        service.stats().registry.retiring,
        1,
        "waiting for the drain"
    );
    assert_eq!(service.finish_teardown().finished, 1);
    assert_eq!(service.stats().loaders.referenced_at_teardown, 0);
    assert_zero_separately(&service);
    teardown(&service);
}

/// (c) A late backend `retired` after shutdown finishes the retirement inline.
#[test]
fn a_late_backend_proof_after_the_pool_ended_finishes_the_retirement_inline() {
    let (_scratch, service) = rig(1, Mode::Backend);
    let key = service.load("small.png").expect("admitted");
    let reference = service.retain(key).expect("a reference");
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    wait_for_status(&service, key, "the job ran", |s| {
        s.state == StatusState::Uploading
    });
    let mut fake = Backend::default();
    fake.take_all(&service);
    let report = pool.shutdown(BOUND);
    assert!(report.unfinished.is_empty(), "{report:?}");
    // `close` ended public ownership everywhere; the reference is all that is left.
    drop(reference);
    fake.release_all();
    assert_eq!(
        service.finish_teardown().finished,
        0,
        "the backend's proof is awaited"
    );
    service.retired(key);
    assert_eq!(service.stats().loaders.inline_finishes, 1);
    assert_zero_separately(&service);
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// A failure of a loader's loop
// ---------------------------------------------------------------------------------------------

/// A panic in a loader's loop outside any job, with a payload whose `Drop` panics, before shutdown: the failure is
/// reported in `LoaderShutdown` and `stats()`, the payload is disposed of on the loader thread, the thread returns
/// normally (joined `Ok`), and no work is claimed for it: the other loader runs every job.
#[test]
fn a_panic_in_a_loaders_loop_is_reported_disposed_of_on_its_thread_and_claims_no_work() {
    let (_scratch, service) = rig(2, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let drops = Arc::new(AtomicUsize::new(0));
    let dropped_on = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::new(AtomicBool::new(false));
    {
        let (drops, dropped_on, fired) = (
            Arc::clone(&drops),
            Arc::clone(&dropped_on),
            Arc::clone(&fired),
        );
        script.on(Stage::LoopTop, None, move |_| {
            if !fired.swap(true, Ordering::SeqCst) {
                std::panic::panic_any(Bomb::new(&drops, &dropped_on));
            }
        });
    }
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    wait_until("the failure was recorded", || {
        service.stats().loaders.loop_failures == 1
    });
    let keys: Vec<TextureKey> = (0..5).map(|_| cpu(&service)).collect();
    for key in &keys {
        wait_for_status(&service, *key, "the other loader ran the job", |s| {
            ready(s.state)
        });
    }
    let report = pool.shutdown(BOUND);
    assert!(report.unfinished.is_empty(), "{report:?}");
    assert_eq!(
        report.finished.len(),
        2,
        "both threads ended and were joined: {report:?}"
    );
    assert_eq!(report.loop_failures.len(), 1);
    assert_eq!(report.loop_failures[0].message, NON_STRING_PAYLOAD);
    assert!(report.loop_failures[0].thread.starts_with("ck-loader-"));
    assert_eq!(drops.load(Ordering::SeqCst), 1, "disposed of");
    let ran_on = dropped_on
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_ne!(ran_on[0], thread::current().id(), "on the loader thread");
    let _ = service.finish_teardown();
    teardown(&service);
}

/// The same, after shutdown has detached that loader: it finishes its held job, fails at the top of its loop, disposes of
/// the payload on its own thread and ends, without aborting the process.
#[test]
fn a_loop_failure_after_the_loader_was_detached_ends_the_thread_without_aborting() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = cpu(&service);
    let held = script.hold(Stage::Run, Some(key));
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, Some(key), 1);
    let report = pool.shutdown(Duration::ZERO);
    assert_eq!(report.unfinished.len(), 1, "{report:?}");
    let drops = Arc::new(AtomicUsize::new(0));
    let dropped_on = Arc::new(Mutex::new(Vec::new()));
    {
        let (drops, dropped_on) = (Arc::clone(&drops), Arc::clone(&dropped_on));
        script.on(Stage::LoopTop, None, move |_| {
            std::panic::panic_any(Bomb::new(&drops, &dropped_on));
        });
    }
    held.open();
    wait_until("the payload was disposed of", || {
        drops.load(Ordering::SeqCst) == 1
    });
    wait_until("the failure was counted", || {
        service.stats().loaders.loop_failures == 1
    });
    let ran_on = dropped_on
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_ne!(ran_on[0], thread::current().id());
    assert_eq!(
        service.stats().cpu_pixels.reserved,
        0,
        "the late result returned its bytes"
    );
    let _ = service.finish_teardown();
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// A bounded end
// ---------------------------------------------------------------------------------------------

/// With a job held in `run`, a short budget returns within the budget plus a small bound, reports that thread unfinished
/// and the others finished; releasing the gate afterwards completes the job, whose result is discarded and whose CPU
/// bytes return; nothing else changes. After shutdown `load` and `create_from_pixels` are `Ended`. Every budget is back to
/// zero once the late job and the fake backend are done and `finish_teardown` has run.
#[test]
fn shutdown_with_a_held_job_reports_it_unfinished_and_its_late_result_is_discarded() {
    let (_scratch, service) = rig(2, Mode::Backend);
    let script = Script::default();
    script.install(&service);
    let file = service.load("small.png").expect("admitted");
    let held = script.hold(Stage::Run, Some(file));
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, Some(file), 1);
    // A populated registry and queue meanwhile: queued requests the close drops.
    let queued: Vec<TextureKey> = (0..30).map(|_| cpu(&service)).collect();
    let started = Instant::now();
    let within = Duration::from_millis(100);
    let report = pool.shutdown(within);
    let took = started.elapsed();
    assert!(
        took < within + Duration::from_secs(5),
        "{took:?}: the budget plus a small bound"
    );
    assert_eq!(report.unfinished.len(), 1, "{report:?}");
    assert_eq!(report.finished.len(), 1, "{report:?}");
    assert!(report.closed_in < BOUND && report.waited < within + Duration::from_secs(5));
    // The cost of `close` and the wait for the threads are told apart, and the wait used the deadline's remainder: the
    // held thread cannot finish, so the two together reach the budget.
    assert!(
        report.closed_in + report.waited + Duration::from_millis(5) >= within,
        "{report:?}"
    );
    assert_eq!(service.load("small.png").unwrap_err(), AdmitError::Ended);
    assert!(matches!(
        service
            .create_from_pixels(replacement(&service))
            .map_err(|refused| refused.error),
        Err(AdmitError::Ended)
    ));
    assert_eq!(
        service.stats().cpu_pixels.reserved,
        0,
        "the queued requests' bytes returned at close"
    );
    let before = status(&service, file);
    held.open();
    wait_until("the late job's slot returned", || {
        service.stats().jobs.reserved == 0
    });
    let after = status(&service, file);
    assert_eq!(
        (after.requested, after.shown, after.committed, after.error),
        (before.requested, None, None, None),
        "nothing else changed: the late result touched no content, readiness or error"
    );
    assert_eq!(
        after.state,
        StatusState::Pending,
        "only its executing bookkeeping cleared"
    );
    let mut fake = Backend::default();
    assert_eq!(fake.take_all(&service), 0, "and no upload was queued");
    drop(queued);
    service.detach_backend();
    let _ = service.finish_teardown();
    wait_until_budgets_zero(&service);
    assert_zero_separately(&service);
    teardown(&service);
}

/// With no held job every thread is reported finished; a zero budget with a held job makes one observation and returns
/// at once.
#[test]
fn shutdown_reports_every_idle_thread_finished_and_a_zero_budget_makes_one_observation() {
    let (_scratch, service) = rig(3, Mode::Headless);
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    assert_eq!(pool.threads(), 3);
    let report = pool.shutdown(BOUND);
    assert_eq!(report.finished.len(), 3, "{report:?}");
    assert!(report.unfinished.is_empty());
    let second = pool.shutdown(BOUND);
    assert!(
        second.finished.is_empty() && second.unfinished.is_empty(),
        "nothing left: {second:?}"
    );
    teardown(&service);

    let (_scratch, service, mut pool, gate) = held_job();
    let started = Instant::now();
    let report = pool.shutdown(Duration::ZERO);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "one observation, no waiting"
    );
    assert_eq!(report.unfinished.len(), 1, "{report:?}");
    gate.open();
    wait_until("the late job ended", || service.stats().jobs.reserved == 0);
    let _ = service.finish_teardown();
    wait_until_budgets_zero(&service);
    teardown(&service);
}

/// A headless pool of one with a job held at its start: `(root, service, pool, gate)`.
fn held_job() -> (Scratch, ResourceService, LoaderPool, Latch) {
    let (scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = cpu(&service);
    let gate = script.hold(Stage::Run, Some(key));
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, Some(key), 1);
    // The script lives as long as the service's hook does.
    (scratch, service, pool, gate)
}

/// A thread that cannot be started: the ones already started are stopped and observed finished within the fixed
/// clean-up budget, and any not finished by then are detached and named in the error.
#[test]
fn a_failed_spawn_stops_the_threads_already_started_and_names_any_unfinished() {
    // Idle threads: all observed finished within the budget.
    let (_scratch, service) = rig(3, Mode::Headless);
    let error = LoaderPool::start_with(service.clone(), &|index, body| {
        if index == 2 {
            Err(io::Error::other("no more threads"))
        } else {
            thread::Builder::new()
                .name(format!("ck-loader-{index}"))
                .spawn(body)
        }
    })
    .expect_err("the third thread cannot start");
    assert_eq!(error.index, 2);
    assert!(error.unfinished.is_empty(), "{error}");
    assert!(error.to_string().contains("no more threads"), "{error}");
    teardown(&service);

    // A started thread held in a job is not finished when the budget ends: detached and named.
    let (_scratch, service) = rig(3, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let key = cpu(&service);
    let held = script.hold(Stage::Run, Some(key));
    let started = Instant::now();
    let error = LoaderPool::start_with(service.clone(), &|index, body| {
        if index == 2 {
            // Let the first thread take the job before the failure.
            script.wait_for(Stage::Run, Some(key), 1);
            Err(io::Error::other("no more threads"))
        } else {
            thread::Builder::new()
                .name(format!("ck-loader-{index}"))
                .spawn(body)
        }
    })
    .expect_err("the third thread cannot start");
    assert!(
        started.elapsed() >= SPAWN_CLEANUP,
        "the fixed budget was waited out"
    );
    assert!(started.elapsed() < SPAWN_CLEANUP + Duration::from_secs(10));
    assert_eq!(error.unfinished.len(), 1, "{error}");
    assert!(
        error.to_string().contains(&error.unfinished[0]),
        "named: {error}"
    );
    held.open();
    // The completion's own acknowledgement (its discards and ticket are dropped), not the slot count: the live
    // resource's metadata stays charged until the teardown retires it.
    script.wait_for(Stage::Completed, Some(key), 1);
    assert_eq!(service.stats().jobs.reserved, 0, "the job's slot is back");
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Locks
// ---------------------------------------------------------------------------------------------

/// No lock is held while a job runs, a file is read or a decode runs: with a CPU job held at its start, and a file job
/// held between the decoder's header checks and its allocation, caller-side calls complete meanwhile (on another
/// thread, with a bounded wait: a held lock would show as a call that never returns).
#[test]
fn the_service_lock_is_free_while_a_job_runs_and_while_a_file_decodes() {
    let (_scratch, service) = rig(2, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let cpu_key = cpu(&service);
    let file_key = service.load("small.png").expect("admitted");
    let cpu_hold = script.hold(Stage::Run, Some(cpu_key));
    let file_hold = script.hold(Stage::BeforeAttempt, Some(file_key));
    let pool = LoaderPool::start(service.clone()).expect("a pool");
    script.wait_for(Stage::Run, Some(cpu_key), 1);
    script.wait_for(Stage::BeforeAttempt, Some(file_key), 1);
    let done = Arc::new(AtomicBool::new(false));
    let caller = {
        let (service, done) = (service.clone(), Arc::clone(&done));
        thread::spawn(move || {
            let _ = service.status(cpu_key);
            let _ = service.stats();
            let other = service.load("small.png").expect("admitted meanwhile");
            let extra = service.reserve_pixels(2, 2).expect("reserved meanwhile");
            drop(extra);
            let _ = service.release(other);
            done.store(true, Ordering::SeqCst);
        })
    };
    wait_until(
        "the caller-side calls completed while both jobs were held",
        || done.load(Ordering::SeqCst),
    );
    caller.join().expect("the caller");
    assert_eq!(state(&service, cpu_key), StatusState::Decoding);
    assert_eq!(state(&service, file_key), StatusState::Decoding);
    cpu_hold.open();
    file_hold.open();
    wait_for_status(&service, cpu_key, "the held CPU job finished", |s| {
        ready(s.state)
    });
    wait_for_status(&service, file_key, "the held file job finished", |s| {
        ready(s.state)
    });
    end(pool, &service, &script);
}

/// A race-free sanity check of the set used above.
#[test]
fn a_hashset_of_thread_names_is_what_the_reports_use() {
    let names: HashSet<String> = (0..3).map(|n| format!("ck-loader-{n}")).collect();
    assert_eq!(names.len(), 3);
    let _ = (
        ServiceWakes::default(),
        reserved as fn(&ResourceService) -> [u64; 3],
    );
}

/// A payload whose drop stalls until released (the arbitrary `Drop` code the loader disposes of on its own thread).
struct Stalled {
    entered: Latch,
    release: Latch,
}

impl Drop for Stalled {
    fn drop(&mut self) {
        self.entered.open();
        self.release.wait();
    }
}

/// The loop failure is published (counted, with its bounded message) **before** the payload is disposed of: while the
/// disposal stalls, shutdown reports the thread unfinished and still reports and counts the failure exactly once.
#[test]
fn a_stalled_payload_disposal_does_not_hide_the_caught_loop_failure() {
    let (_scratch, service) = rig(1, Mode::Headless);
    let script = Script::default();
    script.install(&service);
    let (entered, release) = (Latch::default(), Latch::default());
    {
        let (entered, release) = (entered.clone(), release.clone());
        let fired = Arc::new(AtomicBool::new(false));
        script.on(Stage::LoopTop, None, move |_| {
            if !fired.swap(true, Ordering::SeqCst) {
                std::panic::panic_any(Stalled {
                    entered: entered.clone(),
                    release: release.clone(),
                });
            }
        });
    }
    let mut pool = LoaderPool::start(service.clone()).expect("a pool");
    entered.wait();
    let report = pool.shutdown(Duration::ZERO);
    assert_eq!(
        report.unfinished.len(),
        1,
        "the disposing thread is not finished: {report:?}"
    );
    assert_eq!(
        report.loop_failures.len(),
        1,
        "the failure is reported while disposal is held: {report:?}"
    );
    assert_eq!(report.loop_failures[0].message, NON_STRING_PAYLOAD);
    assert_eq!(service.stats().loaders.loop_failures, 1, "and counted once");
    release.open();
    let _ = service.finish_teardown();
    teardown(&service);
}

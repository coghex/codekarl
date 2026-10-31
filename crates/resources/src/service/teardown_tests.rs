//! CK-108: inline finishing after the pool's shutdown, and `finish_teardown`'s drain (CK-106's drain-before-finish).
//!
//! These tests keep the service handle, make no incidental `maintain` call, and drive the completion by hand (the loader
//! pool's own end is in the pool's tests). "Shutdown" here is `close`, which is what the pool's shutdown does to the
//! service. A resource is finished by exactly one of the drain (`finish_teardown`/`maintain`) and the late completion or
//! backend proof; the counters say which.

use std::thread;

use super::*;
use crate::testkit::{
    Backend, Latch, MIB64, Scratch, file_service, limits, reserved, step, teardown, write_png,
};

fn rig(mode: Mode) -> (Scratch, ResourceService) {
    let scratch = Scratch::new();
    write_png(scratch.path(), "small.png", 2, 2, [7, 7, 7, 255]);
    let service = file_service(scratch.path(), mode, limits(1, MIB64));
    (scratch, service)
}

/// A file resource whose job is executing and whose last reference was dropped: `(key, the held job)`. The key is in the
/// registry's queue of retiring keys, not yet drained.
fn retiring_with_a_job_held(service: &ResourceService) -> (TextureKey, Job) {
    let key = service.load("small.png").expect("admitted");
    let reference = service.retain(key).expect("a reference");
    service.release(key).expect("public ownership ends");
    let job = service.take_job().expect("the job executes");
    drop(reference);
    assert_eq!(service.stats().registry.retiring, 1);
    (key, job)
}

fn assert_all_zero(service: &ResourceService, what: &str) {
    let stats = service.stats();
    assert_eq!(stats.jobs.reserved, 0, "{what}: job slots");
    assert_eq!(stats.cpu_pixels.reserved, 0, "{what}: CPU bytes");
    assert_eq!(stats.metadata.reserved, 0, "{what}: metadata");
    assert!(
        service.registry().queued().is_empty(),
        "{what}: the registry's queue is empty"
    );
    assert_eq!(
        stats.registry.retiring, 0,
        "{what}: nothing is still retiring"
    );
}

/// (a) The job completes **before** `finish_teardown`: its key is still in the registry's queue (not drained), so the
/// completion does not attempt a finish (a `NotDrained` refusal never occurs) and only returns its reservations;
/// `finish_teardown` then finishes the key. Negative control: an unconditional inline finish meets `NotDrained` here.
#[test]
fn a_late_completion_before_the_drain_finishes_nothing_and_the_teardown_finishes_the_key() {
    let (_scratch, service) = rig(Mode::Headless);
    let (key, job) = retiring_with_a_job_held(&service);
    service.close();
    service.complete(job.run());
    let stats = service.stats().loaders;
    assert_eq!(
        (stats.inline_finishes, stats.inline_refusals),
        (0, 0),
        "{stats:?}"
    );
    assert_eq!(
        service.stats().registry.retiring,
        1,
        "still waiting for the drain"
    );
    assert_eq!(service.stats().jobs.reserved, 0, "the job slot returned");
    let done = service.finish_teardown();
    assert_eq!(done.finished, 1, "{done:?} for {key:?}");
    assert_all_zero(&service, "(a)");
    assert_eq!(service.stats().loaders.inline_refusals, 0);
    teardown(&service);
}

/// (b) The job completes **after** `finish_teardown` drained the key into the pending-finish set (the executing job
/// blocked it): the completion finishes it inline.
#[test]
fn a_late_completion_after_the_drain_finishes_the_key_inline() {
    let (_scratch, service) = rig(Mode::Headless);
    let (_key, job) = retiring_with_a_job_held(&service);
    service.close();
    let done = service.finish_teardown();
    assert_eq!(done.finished, 0, "a job is still executing");
    assert_eq!(service.stats().registry.retiring, 1);
    service.complete(job.run());
    let stats = service.stats().loaders;
    assert_eq!(
        (stats.inline_finishes, stats.inline_refusals),
        (1, 0),
        "{stats:?}"
    );
    assert_all_zero(&service, "(b)");
    teardown(&service);
}

/// (c) A completion racing `finish_teardown`'s drain: exactly one of them finishes the key, never two, never none.
#[test]
fn a_completion_racing_the_drain_and_the_drain_finish_the_key_exactly_once() {
    for round in 0..40 {
        let (_scratch, service) = rig(Mode::Headless);
        let (_key, job) = retiring_with_a_job_held(&service);
        service.close();
        let start = Latch::default();
        let completing = {
            let (service, start) = (service.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                service.complete(job.run());
            })
        };
        let draining = {
            let (service, start) = (service.clone(), start.clone());
            thread::spawn(move || {
                start.wait();
                service.finish_teardown().finished
            })
        };
        start.open();
        completing.join().expect("the completion");
        let by_the_drain = draining.join().expect("the drain");
        let stats = service.stats().loaders;
        assert_eq!(
            stats.inline_finishes + by_the_drain as u64,
            1,
            "round {round}: exactly one finish ({stats:?}, drain {by_the_drain})"
        );
        assert_eq!(stats.inline_refusals, 0, "round {round}");
        // A second drain finds nothing left.
        assert_eq!(service.finish_teardown().finished, 0);
        assert_all_zero(&service, "(c)");
        teardown(&service);
    }
}

/// (d) and (e): a late backend `retired` before and after the drain, likewise. A resource with a backend waits for its
/// proof: before the drain the proof only records itself (the drain then finishes the key); after the drain the proof is
/// the last thing the key waited for, and finishes it inline.
#[test]
fn a_late_backend_proof_before_and_after_the_drain_finishes_the_key_exactly_once() {
    for proof_after_the_drain in [false, true] {
        let (_scratch, service) = rig(Mode::Backend);
        let key = service.load("small.png").expect("admitted");
        let reference = service.retain(key).expect("a reference");
        step(&service).expect("the job");
        let mut fake = Backend::default();
        fake.take_all(&service);
        service.release(key).expect("public ownership ends");
        drop(reference);
        fake.release_all();
        service.close();
        if proof_after_the_drain {
            let done = service.finish_teardown();
            assert_eq!(done.finished, 0, "the backend's proof is awaited");
            service.retired(key);
            let stats = service.stats().loaders;
            assert_eq!(
                (stats.inline_finishes, stats.inline_refusals),
                (1, 0),
                "{stats:?}"
            );
        } else {
            service.retired(key);
            assert_eq!(
                service.stats().loaders.inline_finishes,
                0,
                "not drained: left to the drain"
            );
            let done = service.finish_teardown();
            assert_eq!(done.finished, 1);
        }
        assert_eq!(service.stats().loaders.inline_refusals, 0);
        assert_all_zero(&service, if proof_after_the_drain { "(e)" } else { "(d)" });
        teardown(&service);
    }
}

/// `finish_teardown` reports how many entries were still referenced when it ran: the case outside its contract is
/// visible, not silent.
#[test]
fn finish_teardown_counts_the_entries_still_referenced() {
    let (_scratch, service) = rig(Mode::Headless);
    let key = service.load("small.png").expect("admitted");
    let reference = service.retain(key).expect("a reference");
    while step(&service).is_some() {}
    service.close();
    let done = service.finish_teardown();
    assert_eq!(done.finished, 0);
    assert_eq!(service.stats().loaders.referenced_at_teardown, 1);
    drop(reference);
    // Dropped after the teardown: outside its contract, and `maintain` still cleans it up.
    assert_eq!(service.maintain(8).finished, 1);
    assert_eq!(reserved(&service), [0, 0, 0]);
    teardown(&service);
}

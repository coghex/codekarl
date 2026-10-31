//! CK-107: the resource service, stepped by hand. The tests follow the transition table of the specification (T1 to
//! T19); the doc comment of each names its rows. Every test ends by closing the service, retiring everything and asserting
//! all three budgets are back to zero reserved, and that no value was dropped with a lock of this crate held.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::convert::premultiply;
use crate::lockdebug;

const BOUND: Duration = Duration::from_secs(10);

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < BOUND, "gave up waiting for {what}");
        thread::yield_now();
    }
}

fn limits(max_textures: u32, max_pending_jobs: u32) -> ResourceLimits {
    ResourceLimits {
        max_textures,
        max_pending_jobs,
        ..ResourceLimits::DEFAULT
    }
}

fn service(mode: Mode) -> ResourceService {
    ResourceService::new(ResourceLimits::DEFAULT, mode).expect("a service")
}

fn small(max_textures: u32, max_pending_jobs: u32, mode: Mode) -> ResourceService {
    ResourceService::new(limits(max_textures, max_pending_jobs), mode).expect("a service")
}

/// A straight-alpha, fully opaque `width` x `height` source.
fn opaque(service: &ResourceService, width: u32, height: u32) -> CpuPixels {
    let reservation = service.reserve_pixels(width, height).expect("reserved");
    let bytes = vec![255; reservation.byte_len()];
    CpuPixels::straight_alpha(reservation, bytes).expect("the right length")
}

/// A source whose bytes are all `value`.
fn filled(service: &ResourceService, width: u32, height: u32, value: u8) -> CpuPixels {
    let reservation = service.reserve_pixels(width, height).expect("reserved");
    let bytes = vec![value; reservation.byte_len()];
    CpuPixels::premultiplied(reservation, bytes).expect("the right length")
}

fn create(service: &ResourceService) -> TextureKey {
    service
        .create_from_pixels(opaque(service, 2, 2))
        .expect("admitted")
}

fn replace(service: &ResourceService, key: TextureKey) -> u64 {
    service
        .replace_pixels(key, opaque(service, 2, 2))
        .expect("admitted")
}

/// Takes the next job, runs it on this thread and completes it.
fn step(service: &ResourceService) -> Option<(TextureKey, u64)> {
    let job = service.take_job()?;
    let named = (job.key(), job.revision());
    service.complete(job.run());
    Some(named)
}

fn uploads(service: &ResourceService) -> Vec<UploadRequest> {
    let mut out = Vec::new();
    service.take_uploads(&mut out, usize::MAX);
    out
}

fn status(service: &ResourceService, key: TextureKey) -> TextureStatus {
    service.status(key).expect("a status")
}

/// The fake backend, in one call: takes what is waiting, commits it and acknowledges it. Returns the `Arc`s it kept.
fn backend_adopts(service: &ResourceService) -> Vec<Arc<PixelRevision>> {
    let mut kept = Vec::new();
    for upload in uploads(service) {
        if service.try_commit(upload.key, upload.revision) == CommitOutcome::Committed {
            service.adopted(upload.key, upload.revision);
        }
        kept.push(upload.pixels);
    }
    kept
}

fn reserved(service: &ResourceService) -> [u64; 3] {
    let stats = service.stats();
    [
        stats.jobs.reserved,
        stats.cpu_pixels.reserved,
        stats.metadata.reserved,
    ]
}

/// Closes, retires everything (running every job, retiring through the fake backend) and asserts the budgets are zero and
/// that no value was dropped with a lock held.
fn teardown(service: &ResourceService) {
    service.close();
    for _ in 0..8 {
        while step(service).is_some() {}
        let mut keys = Vec::new();
        service.take_retirements(&mut keys, usize::MAX);
        for key in keys {
            service.retired(key);
        }
        let _ = service.maintain(usize::MAX);
    }
    assert_eq!(reserved(service), [0, 0, 0], "{:?}", service.stats());
    assert_eq!(
        lockdebug::violations(),
        0,
        "a value was dropped with a lock held"
    );
}

/// Runs `maintain` and the fake backend's retirements until `wanted` resources have finished, in at most 50 rounds (a
/// regression fails the test instead of hanging it).
fn retire_until(service: &ResourceService, wanted: usize) {
    let mut finished = 0;
    for _ in 0..50 {
        finished += service.maintain(8).finished;
        let mut keys = Vec::new();
        service.take_retirements(&mut keys, 8);
        for key in keys {
            service.retired(key);
        }
        if finished >= wanted {
            return;
        }
    }
    panic!("only {finished} of {wanted} retirements finished");
}

/// Counts wake calls, and notes whether a lock was held when one ran.
#[derive(Clone, Default)]
struct Calls {
    jobs: Arc<AtomicUsize>,
    uploads: Arc<AtomicUsize>,
    retirements: Arc<AtomicUsize>,
    under_lock: Arc<AtomicUsize>,
}

impl Calls {
    fn wakes(&self) -> ServiceWakes {
        let make = |count: &Arc<AtomicUsize>| -> ServiceWake {
            let (count, under_lock) = (Arc::clone(count), Arc::clone(&self.under_lock));
            Arc::new(move || {
                if lockdebug::held() {
                    under_lock.fetch_add(1, Ordering::SeqCst);
                }
                count.fetch_add(1, Ordering::SeqCst);
            })
        };
        ServiceWakes {
            jobs: Some(make(&self.jobs)),
            uploads: Some(make(&self.uploads)),
            retirements: Some(make(&self.retirements)),
        }
    }

    fn service(&self, mode: Mode) -> ResourceService {
        ResourceService::with_wakes(ResourceLimits::DEFAULT, mode, self.wakes()).expect("a service")
    }

    fn counts(&self) -> [usize; 3] {
        [
            self.jobs.load(Ordering::SeqCst),
            self.uploads.load(Ordering::SeqCst),
            self.retirements.load(Ordering::SeqCst),
        ]
    }
}

// ---------------------------------------------------------------------------------------------
// CPU pixel sources
// ---------------------------------------------------------------------------------------------

#[test]
fn sides_up_to_8192_and_64_mib_are_accepted_and_anything_more_is_refused_before_any_reservation() {
    let service = service(Mode::Backend);
    for (width, height) in [(8192, 2048), (4096, 4096), (1, 8192), (8192, 1), (1, 1)] {
        let reservation = service.reserve_pixels(width, height).expect("accepted");
        assert_eq!(reservation.byte_len(), width as usize * height as usize * 4);
    }
    assert_eq!(
        reserved(&service),
        [0, 0, 0],
        "the reservations were dropped"
    );
    let before = service.stats();
    for (width, height) in [
        (8193, 1),
        (1, 8193),
        (0, 1),
        (1, 0),
        (8192, 2049),
        (8192, 8192),
        (u32::MAX, u32::MAX),
    ] {
        assert_eq!(
            service.reserve_pixels(width, height).unwrap_err(),
            AdmitError::TooLarge { width, height },
            "{width}x{height}"
        );
    }
    assert_eq!(
        service.stats().cpu_pixels,
        before.cpu_pixels,
        "no reservation was tried or counted"
    );
    teardown(&service);
}

#[test]
fn a_reservation_beyond_the_cpu_budget_is_refused_naming_the_numbers() {
    let service = service(Mode::Backend);
    let mut held = Vec::new();
    // 512 MiB / 64 MiB = 8 images at the decoder's limit.
    for _ in 0..8 {
        held.push(service.reserve_pixels(4096, 4096).expect("fits"));
    }
    let error = service.reserve_pixels(1, 1).unwrap_err();
    assert_eq!(
        error,
        AdmitError::Capacity {
            budget: BudgetKind::CpuPixelBytes,
            requested: 4,
            available: 0
        }
    );
    assert_eq!(service.stats().cpu_pixels.refusals, 1);
    drop(held);
    assert_eq!(reserved(&service), [0, 0, 0]);
    teardown(&service);
}

#[test]
fn a_wrong_buffer_length_is_refused_with_the_reservation_and_the_buffer_returned() {
    let service = service(Mode::Backend);
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let error = CpuPixels::straight_alpha(reservation, vec![0; 15])
        .map(|_| ())
        .unwrap_err();
    assert_eq!((error.bytes.len(), error.expected), (15, 16));
    assert_eq!(
        reserved(&service)[1],
        16,
        "the reservation came back with the error"
    );
    let again = CpuPixels::premultiplied(error.reservation, vec![0; 16]).expect("fits now");
    assert_eq!((again.width(), again.height()), (2, 2));
    drop(again);
    assert_eq!(reserved(&service), [0, 0, 0]);
    teardown(&service);
}

/// T1, T4, T5: straight-alpha pixels are premultiplied in the job with the decoder's rule.
#[test]
fn straight_alpha_pixels_are_premultiplied_in_the_job_exactly_as_the_decoder_does() {
    let service = service(Mode::Backend);
    let reservation = service.reserve_pixels(4, 1).expect("reserved");
    let source = [
        [200, 100, 50, 127],
        [200, 100, 50, 128],
        [255, 255, 255, 0],
        [9, 8, 7, 255],
    ];
    let bytes: Vec<u8> = source.iter().flatten().copied().collect();
    let key = service
        .create_from_pixels(CpuPixels::straight_alpha(reservation, bytes).expect("length"))
        .expect("admitted");
    step(&service).expect("the job ran");
    let upload = uploads(&service).pop().expect("a finished revision");
    let expected: Vec<u8> = source
        .iter()
        .flat_map(|[r, g, b, a]| {
            if *a == 0 {
                [0, 0, 0, 0]
            } else {
                [
                    premultiply(*r, *a),
                    premultiply(*g, *a),
                    premultiply(*b, *a),
                    *a,
                ]
            }
        })
        .collect();
    assert_eq!(upload.pixels.pixels(), expected.as_slice());
    assert_eq!(
        &expected[0..4],
        &[100, 50, 25, 127],
        "(c * a + 127) / 255 at alpha 127"
    );
    assert_eq!(&expected[4..8], &[100, 50, 25, 128], "and at alpha 128");
    assert_eq!(
        &expected[8..12],
        &[0, 0, 0, 0],
        "a transparent pixel is all zeros"
    );
    assert_eq!((upload.pixels.key(), upload.pixels.revision()), (key, 1));
    drop(upload);
    teardown(&service);
}

#[test]
fn a_premultiplied_buffer_is_passed_through_byte_for_byte() {
    let service = service(Mode::Backend);
    let reservation = service.reserve_pixels(2, 1).expect("reserved");
    let bytes = vec![10, 5, 1, 100, 0, 0, 0, 0];
    let key = service
        .create_from_pixels(CpuPixels::premultiplied(reservation, bytes.clone()).expect("length"))
        .expect("admitted");
    step(&service).expect("the job ran");
    let upload = uploads(&service).pop().expect("a finished revision");
    assert_eq!(upload.pixels.pixels(), bytes.as_slice());
    assert_eq!((upload.key, upload.revision), (key, 1));
    drop(upload);
    teardown(&service);
}

/// T7: a premultiplied buffer with a colour above its alpha fails its job, naming the first such pixel.
#[test]
fn a_premultiplied_pixel_with_a_colour_above_its_alpha_fails_the_job_naming_its_position() {
    let service = service(Mode::Backend);
    let reservation = service.reserve_pixels(3, 2).expect("reserved");
    let mut bytes = vec![0; 3 * 2 * 4];
    bytes[4 * 4..4 * 4 + 4].copy_from_slice(&[10, 0, 0, 9]); // pixel 4 = (x 1, y 1)
    bytes[5 * 4..5 * 4 + 4].copy_from_slice(&[255, 0, 0, 1]); // a later one
    let key = service
        .create_from_pixels(CpuPixels::premultiplied(reservation, bytes).expect("length"))
        .expect("admitted");
    step(&service).expect("the job ran");
    let report = status(&service, key);
    assert_eq!(report.state, StatusState::Failed);
    let error = report.error.expect("an error");
    assert_eq!(error.revision, 1);
    assert!(error.message().contains("(1, 1)"), "{}", error.message());
    assert!(
        uploads(&service).is_empty(),
        "a failed revision is not offered"
    );
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Foreign reservations (F3), T19
// ---------------------------------------------------------------------------------------------

/// T19, F3: pixels reserved from another service are refused with their own reservation handed back.
#[test]
fn pixels_reserved_from_another_service_are_refused_and_handed_back_with_their_reservation() {
    for (a_limits, b_limits) in [
        (ResourceLimits::DEFAULT, ResourceLimits::DEFAULT),
        (limits(8, 8), ResourceLimits::DEFAULT),
    ] {
        let a = ResourceService::new(a_limits, Mode::Backend).expect("a");
        let b = ResourceService::new(b_limits, Mode::Backend).expect("b");
        assert_ne!(a.ledger_id(), b.ledger_id());
        let own = create(&b);
        let before = b.stats();
        let foreign = opaque(&a, 2, 2);
        let refused = b
            .create_from_pixels(foreign)
            .expect_err("a foreign reservation");
        assert_eq!(refused.error, AdmitError::ForeignReservation);
        assert_eq!(b.stats(), before, "B's counters are unchanged");
        let refused_again = b
            .replace_pixels(own, refused.pixels)
            .expect_err("foreign for a replacement too");
        assert_eq!(refused_again.error, AdmitError::ForeignReservation);
        assert_eq!(b.stats(), before);
        assert_eq!(reserved(&a)[1], 16, "A's reservation is still A's");
        drop(refused_again.pixels);
        assert_eq!(
            reserved(&a)[1],
            0,
            "A's budget returns exactly once when the caller drops them"
        );
        assert_eq!(b.stats(), before);
        teardown(&a);
        teardown(&b);
    }
}

#[test]
fn pixels_reserved_through_a_clone_of_the_service_are_accepted() {
    let service = service(Mode::Backend);
    let clone = service.clone();
    assert_eq!(service.ledger_id(), clone.ledger_id());
    let key = service
        .create_from_pixels(opaque(&clone, 2, 2))
        .expect("the same ledger");
    assert_eq!(
        service
            .replace_pixels(key, opaque(&clone, 2, 2))
            .expect("admitted"),
        2
    );
    teardown(&clone);
}

// ---------------------------------------------------------------------------------------------
// Admission: T1, T2, T3, T19
// ---------------------------------------------------------------------------------------------

/// T1: a created resource has revision 1 queued, is `pending`, and its charges are the pixels, one job slot and the
/// metadata of a resource and of a job.
#[test]
fn t01_create_queues_revision_one_and_charges_the_pixels_a_job_slot_and_the_metadata() {
    let service = service(Mode::Backend);
    let key = create(&service);
    let report = status(&service, key);
    assert_eq!(report.state, StatusState::Pending);
    assert_eq!(
        (report.requested, report.committed, report.shown),
        (1, None, None)
    );
    assert_eq!(
        reserved(&service),
        [1, 16, RESOURCE_METADATA_BYTES + JOB_METADATA_BYTES]
    );
    assert_eq!(service.queue_lengths(), [1, 0, 0, 0]);
    assert!(RESOURCE_METADATA_BYTES.is_multiple_of(64) && JOB_METADATA_BYTES.is_multiple_of(64));
    assert!(RESOURCE_METADATA_BYTES >= size_of::<ResourceState>() as u64);
    assert!(JOB_METADATA_BYTES >= (size_of::<Request>() + size_of::<JobOutput>()) as u64);
    teardown(&service);
}

/// T19: each of the registry, the job slots, the metadata and the CPU bytes is refused in turn with nothing changed, and
/// the caller gets the pixels (and their reservation) back.
#[test]
fn t19_each_refused_admission_changes_nothing_and_hands_the_pixels_back() {
    // Registry slots.
    let service = small(1, 8, Mode::Backend);
    let key = create(&service);
    let before = service.stats();
    let pixels = opaque(&service, 2, 2);
    let held = reserved(&service);
    let refused = service.create_from_pixels(pixels).expect_err("no slot");
    assert_eq!(refused.error, AdmitError::RegistryFull);
    assert_eq!(refused.pixels.width(), 2);
    drop(refused);
    let after = service.stats();
    assert_eq!(
        (after.jobs.reserved, after.metadata.reserved, after.registry),
        (
            before.jobs.reserved,
            before.metadata.reserved,
            before.registry
        ),
        "nothing changed (the caller's own 16 bytes were returned when it dropped them)"
    );
    assert_eq!(held[1], 32);
    assert_eq!(reserved(&service)[1], 16);
    let _ = key;
    teardown(&service);

    // Job slots.
    let service = small(8, 1, Mode::Backend);
    create(&service);
    let before = service.stats();
    let refused = service
        .create_from_pixels(opaque(&service, 2, 2))
        .expect_err("no job slot");
    assert_eq!(refused.error, AdmitError::JobsFull);
    drop(refused);
    let after = service.stats();
    assert_eq!(
        (after.jobs.reserved, after.metadata.reserved),
        (1, before.metadata.reserved)
    );
    assert_eq!(after.registry, before.registry);
    teardown(&service);

    // Metadata bytes: 1 MiB is the smallest budget.
    let service = ResourceService::new(
        ResourceLimits {
            metadata_bytes: 1024 * 1024,
            ..ResourceLimits::DEFAULT
        },
        Mode::Backend,
    )
    .expect("a service");
    let per_resource = RESOURCE_METADATA_BYTES + JOB_METADATA_BYTES;
    let fits = 1024 * 1024 / per_resource;
    for _ in 0..fits {
        create(&service);
    }
    let before = service.stats();
    let refused = service
        .create_from_pixels(opaque(&service, 1, 1))
        .expect_err("no metadata");
    assert!(
        matches!(
            refused.error,
            AdmitError::Capacity {
                budget: BudgetKind::MetadataBytes,
                ..
            }
        ),
        "{:?}",
        refused.error
    );
    drop(refused);
    let after = service.stats();
    assert_eq!(
        (after.jobs.reserved, after.metadata.reserved, after.registry),
        (
            before.jobs.reserved,
            before.metadata.reserved,
            before.registry
        )
    );
    teardown(&service);

    // CPU bytes: refused when the producer reserves, before it allocates anything.
    let service = service_with_full_cpu();
    assert!(matches!(
        service.reserve_pixels(1, 1),
        Err(AdmitError::Capacity {
            budget: BudgetKind::CpuPixelBytes,
            ..
        })
    ));
}

fn service_with_full_cpu() -> ResourceService {
    let service = ResourceService::new(
        ResourceLimits {
            cpu_pixel_bytes: 64 * 1024 * 1024,
            ..ResourceLimits::DEFAULT
        },
        Mode::Backend,
    )
    .expect("a service");
    let full = service.reserve_pixels(4096, 4096).expect("64 MiB");
    std::mem::forget(full);
    service
}

/// T2: a replacement of a queued request succeeds with the job budget full, drops the old pixels and passes the job slot.
#[test]
fn t02_replacing_a_queued_request_succeeds_with_the_job_budget_full() {
    let service = small(8, 1, Mode::Backend);
    let key = create(&service);
    assert_eq!(reserved(&service)[0], 1);
    let cpu_before = reserved(&service)[1];
    let revision = service
        .replace_pixels(key, opaque(&service, 3, 3))
        .expect("the slot passes to the new request");
    assert_eq!(revision, 2);
    assert_eq!(reserved(&service)[0], 1, "still one job slot");
    assert_eq!(
        reserved(&service)[1],
        36,
        "the old 16 bytes were returned and the new 36 are held (was {cpu_before})"
    );
    assert_eq!(
        service.queue_lengths()[0],
        1,
        "the queue position is the old one: one entry"
    );
    assert_eq!(status(&service, key).requested, 2);
    let (taken, taken_revision) = step(&service).expect("one job");
    assert_eq!((taken, taken_revision), (key, 2));
    assert!(step(&service).is_none(), "revision 1 never ran");
    teardown(&service);
}

/// T3: with none queued a replacement needs a job slot of its own.
#[test]
fn t03_replacing_with_none_queued_needs_a_job_slot() {
    let service = small(8, 1, Mode::Backend);
    let key = create(&service);
    let job = service.take_job().expect("revision 1 executes");
    // Nothing is queued now, and the one job slot is held by the executing job.
    let before = service.stats();
    let refused = service
        .replace_pixels(key, opaque(&service, 2, 2))
        .expect_err("no job slot");
    assert_eq!(refused.error, AdmitError::JobsFull);
    drop(refused);
    assert_eq!(service.stats().jobs.reserved, before.jobs.reserved);
    assert_eq!(
        status(&service, key).requested,
        1,
        "the revision did not advance"
    );
    service.complete(job.run());
    // The slot is back: now it succeeds.
    assert_eq!(replace(&service, key), 2);
    teardown(&service);
}

#[test]
fn a_replacement_needs_live_public_ownership_and_a_service_that_has_not_ended() {
    let service = service(Mode::Backend);
    let key = create(&service);
    let reference = service.retain(key).expect("retained");
    service.release(key).expect("released");
    let refused = service
        .replace_pixels(key, opaque(&service, 2, 2))
        .expect_err("stale");
    assert_eq!(refused.error, AdmitError::Handle(HandleError::Stale(key)));
    drop(refused);
    drop(reference);
    service.close();
    let refused = service
        .create_from_pixels(opaque(&service, 2, 2))
        .expect_err("ended");
    assert_eq!(refused.error, AdmitError::Ended);
    drop(refused);
    let refused = service
        .replace_pixels(key, opaque(&service, 2, 2))
        .expect_err("ended");
    assert_eq!(refused.error, AdmitError::Ended);
    drop(refused);
    teardown(&service);
}

#[test]
fn is_closed_reads_the_closed_flag_for_both_modes_and_close_is_idempotent() {
    for mode in [Mode::Backend, Mode::Headless] {
        let service = service(mode);
        assert!(!service.is_closed());
        let key = create(&service);
        assert!(!service.is_closed(), "creating a resource does not close");
        service.close();
        assert!(service.is_closed());
        service.close();
        assert!(service.is_closed(), "a second close changes nothing");
        // A clone is the same service.
        assert!(service.clone().is_closed());
        let refused = service
            .replace_pixels(key, opaque(&service, 2, 2))
            .expect_err("ended");
        assert_eq!(refused.error, AdmitError::Ended);
        drop(refused);
        teardown(&service);
    }
}

#[test]
fn a_resource_at_the_last_revision_refuses_the_next_request_and_changes_nothing() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("revision 1");
    service.force_requested(key, u64::MAX);
    let before = (service.stats(), status(&service, key));
    let refused = service
        .replace_pixels(key, opaque(&service, 2, 2))
        .expect_err("no revision left");
    assert_eq!(refused.error, AdmitError::RevisionsExhausted);
    drop(refused);
    let after = (service.stats(), status(&service, key));
    assert_eq!(after.1, before.1);
    assert_eq!(after.0.registry, before.0.registry);
    assert_eq!(after.0.jobs.reserved, before.0.jobs.reserved);
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// The executor: T4, T5, T6, T7
// ---------------------------------------------------------------------------------------------

/// T4: jobs come out oldest first across resources, never two at once for one resource.
#[test]
fn t04_jobs_come_out_fifo_and_never_two_at_once_for_one_resource() {
    let service = service(Mode::Backend);
    let (a, b, c) = (create(&service), create(&service), create(&service));
    let first = service.take_job().expect("a");
    assert_eq!((first.key(), first.revision()), (a, 1));
    assert_eq!(status(&service, a).state, StatusState::Decoding);
    // A replacement of `a` while its job executes waits for it; `b` and `c` go first.
    assert_eq!(replace(&service, a), 2);
    let second = service.take_job().expect("b");
    assert_eq!(second.key(), b);
    let third = service.take_job().expect("c");
    assert_eq!(third.key(), c);
    assert!(
        service.take_job().is_none(),
        "`a` has a job executing: its replacement waits"
    );
    // The completion releases the successor.
    service.complete(first.run());
    let successor = service.take_job().expect("a's replacement");
    assert_eq!((successor.key(), successor.revision()), (a, 2));
    service.complete(second.run());
    service.complete(third.run());
    service.complete(successor.run());
    teardown(&service);
}

/// T5: a current result waits for upload (`Backend`).
#[test]
fn t05_a_current_result_waits_for_upload_in_backend_mode() {
    let calls = Calls::default();
    let service = calls.service(Mode::Backend);
    let key = create(&service);
    let job = service.take_job().expect("a job");
    let output = job.run();
    assert!(output.is_ok());
    service.complete(output);
    assert_eq!(reserved(&service)[0], 0, "the job slot returned");
    assert_eq!(
        reserved(&service)[2],
        RESOURCE_METADATA_BYTES,
        "the job metadata returned"
    );
    assert_eq!(reserved(&service)[1], 16, "the revision holds the pixels");
    assert_eq!(status(&service, key).state, StatusState::Uploading);
    assert_eq!(
        calls.counts()[1],
        1,
        "the uploads wake: the queue went non-empty"
    );
    assert_eq!(service.queue_lengths()[1], 1);
    teardown(&service);
}

/// T10 (the revision waiting for upload): a request admitted while a finished revision waits for upload drops it at once.
#[test]
fn a_revision_waiting_for_upload_is_dropped_at_once_when_a_newer_request_is_admitted() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("revision 1 finishes");
    assert_eq!(reserved(&service)[1], 16, "revision 1 waits for upload");
    assert_eq!(service.queue_lengths()[1], 1);
    replace(&service, key); // revision 2
    assert_eq!(
        reserved(&service)[1],
        16,
        "revision 1's bytes returned at once; revision 2's source is held"
    );
    assert!(
        uploads(&service).is_empty(),
        "nothing is offered for the superseded revision"
    );
    step(&service).expect("revision 2 finishes");
    let offered = uploads(&service);
    assert_eq!(offered.iter().map(|u| u.revision).collect::<Vec<_>>(), [2]);
    drop(offered);
    teardown(&service);
}

/// T5 in `Headless`: shown `cpu_ready` at once; never `ready`; the pixels are dropped and their bytes returned.
#[test]
fn t05_headless_shows_cpu_ready_at_once_and_drops_the_pixels() {
    let service = service(Mode::Headless);
    let key = create(&service);
    step(&service).expect("the job");
    let report = status(&service, key);
    assert_eq!(report.state, StatusState::CpuReady);
    assert_eq!(
        report.shown,
        Some(ShownRevision {
            revision: 1,
            width: 2,
            height: 2
        }),
        "the dimensions stay"
    );
    assert_eq!(
        reserved(&service),
        [0, 0, RESOURCE_METADATA_BYTES],
        "the pixels' bytes are back"
    );
    assert!(uploads(&service).is_empty());
    // A replacement shows `cpu_ready` again, still never `ready`.
    replace(&service, key);
    step(&service).expect("the replacement job");
    assert_eq!(status(&service, key).state, StatusState::CpuReady);
    assert_eq!(status(&service, key).shown.map(|s| s.revision), Some(2));
    teardown(&service);
}

/// T6: a job for revision 1 completing after revision 2 was admitted is discarded; its bytes return at once and the
/// successor runs.
#[test]
fn t06_a_superseded_result_is_discarded_its_bytes_return_and_the_successor_runs() {
    let service = service(Mode::Backend);
    let key = create(&service);
    let job = service.take_job().expect("revision 1");
    let output = job.run();
    replace(&service, key); // revision 2 queued behind the executing job
    assert_eq!(reserved(&service)[1], 32, "both sources are held");
    service.complete(output);
    let report = status(&service, key);
    assert_eq!(
        report.state,
        StatusState::Pending,
        "revision 1 left no content, readiness or error"
    );
    assert_eq!(
        (report.shown, report.committed, report.error),
        (None, None, None)
    );
    assert_eq!(
        reserved(&service)[1],
        16,
        "revision 1's pixels returned at once"
    );
    assert!(uploads(&service).is_empty());
    let successor = service
        .take_job()
        .expect("revision 2 is released by the discard");
    assert_eq!(successor.revision(), 2);
    service.complete(successor.run());
    assert_eq!(uploads(&service).len(), 1);
    teardown(&service);
}

/// T6: a result arriving after the resource began retiring is discarded, and the retirement then finishes.
#[test]
fn t06_a_result_for_a_retiring_resource_is_discarded_and_the_retirement_then_finishes() {
    let service = service(Mode::Headless);
    let key = create(&service);
    let job = service.take_job().expect("revision 1 executes");
    service
        .release(key)
        .expect("released: no reference, so retiring");
    let output = job.run();
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 1,
            finished: 0
        },
        "an executing job blocks it"
    );
    service.complete(output);
    assert!(
        service.retirement_pending(),
        "the discarded completion flagged retirement work"
    );
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 0,
            finished: 1
        }
    );
    assert_eq!(reserved(&service), [0, 0, 0], "everything came back");
    assert_eq!(service.status(key), Err(HandleError::Stale(key)));
    teardown(&service);
}

/// T7: a failed job records the error and changes neither the committed nor the shown revision.
#[test]
fn t07_a_failed_job_records_the_error_and_a_later_success_clears_it() {
    let service = service(Mode::Headless);
    let key = service
        .create_from_pixels(filled(&service, 2, 2, 255))
        .expect("admitted");
    step(&service).expect("revision 1 shows");
    // Revision 2: a colour above its alpha.
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bad = CpuPixels::premultiplied(reservation, [200, 0, 0, 100].repeat(4)).expect("length");
    service.replace_pixels(key, bad).expect("admitted");
    step(&service).expect("revision 2 fails");
    let report = status(&service, key);
    assert_eq!(
        report.state,
        StatusState::CpuReady,
        "the last good content stays shown"
    );
    assert_eq!(report.shown.map(|s| s.revision), Some(1));
    let error = report.error.expect("the error");
    assert_eq!(error.revision, 2);
    assert!(matches!(error.text, ErrorText::Stored(_)));
    assert!(reserved(&service)[2] >= RESOURCE_METADATA_BYTES + error.message().len() as u64);
    // Nothing retries on its own, and nothing is reported per frame.
    assert!(step(&service).is_none());
    // A later success clears the error (it is older than the new shown revision).
    replace(&service, key);
    step(&service).expect("revision 3");
    let report = status(&service, key);
    assert_eq!(
        (report.shown.map(|s| s.revision), report.error),
        (Some(3), None)
    );
    assert_eq!(
        reserved(&service)[2],
        RESOURCE_METADATA_BYTES,
        "the error's charge returned"
    );
    teardown(&service);
}

#[test]
fn an_error_text_is_cut_at_a_character_boundary_and_a_refused_charge_stores_a_fixed_text() {
    assert_eq!(truncate("héllo", 2), "h");
    assert_eq!(truncate("héllo", 3), "hé");
    assert_eq!(truncate("abc", 3), "abc");
    assert_eq!(truncate("", 0), "");
    let service = ResourceService::new(
        ResourceLimits {
            metadata_bytes: 1024 * 1024,
            ..ResourceLimits::DEFAULT
        },
        Mode::Headless,
    )
    .expect("a service");
    let key = create(&service);
    // Fill the metadata budget so the error charge is refused.
    let mut fillers = Vec::new();
    while let Ok(reservation) = service.shared.ledger.reserve(BudgetKind::MetadataBytes, 64) {
        fillers.push(reservation);
    }
    let long = "x".repeat(MAX_ERROR_BYTES + 100);
    service.upload_failed(key, 1, &long);
    let report = status(&service, key);
    let error = report.error.expect("an error");
    assert!(
        matches!(error.text, ErrorText::Fixed(_)),
        "no allocation, a fixed text"
    );
    assert!(error.message().contains("not stored"));
    drop(fillers);
    // With room, the text is stored cut to the bound.
    service.upload_failed(key, 1, &long);
    let error = status(&service, key).error.expect("an error");
    assert_eq!(error.message().len(), MAX_ERROR_BYTES);
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// The backend side: T8 to T13
// ---------------------------------------------------------------------------------------------

/// T8: uploads are taken oldest first, at most `limit`, and the resource is `uploading`.
#[test]
fn t08_uploads_are_taken_fifo_up_to_the_limit() {
    let service = service(Mode::Backend);
    let keys: Vec<_> = (0..3).map(|_| create(&service)).collect();
    while step(&service).is_some() {}
    let mut out = Vec::new();
    assert_eq!(service.take_uploads(&mut out, 2), 2);
    assert_eq!(out.iter().map(|u| u.key).collect::<Vec<_>>(), keys[..2]);
    assert_eq!(service.take_uploads(&mut out, 8), 1);
    assert_eq!(out[2].key, keys[2]);
    assert_eq!(service.take_uploads(&mut out, 8), 0);
    assert!(
        out.iter()
            .all(|u| u.revision == 1 && u.pixels.revision() == 1)
    );
    assert_eq!(status(&service, keys[0]).state, StatusState::Uploading);
    assert_eq!(
        reserved(&service)[1],
        48,
        "the backend's Arcs hold the bytes"
    );
    drop(out);
    assert_eq!(reserved(&service)[1], 0, "dropping them returns the bytes");
    teardown(&service);
}

/// T9: a current, taken revision commits; it is committed, not shown.
#[test]
fn t09_commit_makes_the_revision_committed_but_not_shown_until_adopted() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("the job");
    let upload = uploads(&service).pop().expect("a finished revision");
    assert_eq!(
        service.try_commit(key, upload.revision),
        CommitOutcome::Committed
    );
    let report = status(&service, key);
    assert_eq!((report.committed, report.shown), (Some(1), None));
    assert_eq!(
        report.state,
        StatusState::Uploading,
        "committed is not shown"
    );
    // T12: the acknowledgement makes it shown and ready.
    service.adopted(key, 1);
    let report = status(&service, key);
    assert_eq!(report.state, StatusState::Ready);
    assert_eq!(
        report.shown,
        Some(ShownRevision {
            revision: 1,
            width: 2,
            height: 2
        })
    );
    drop(upload);
    teardown(&service);
}

/// T10: a request admitted before the commit makes it rejected.
#[test]
fn t10_a_request_admitted_before_the_commit_makes_it_rejected() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("revision 1");
    let upload = uploads(&service).pop().expect("taken by the backend");
    replace(&service, key); // revision 2, admitted before the commit
    assert_eq!(service.try_commit(key, 1), CommitOutcome::Rejected);
    let report = status(&service, key);
    assert_eq!(
        (report.committed, report.shown, report.requested),
        (None, None, 2)
    );
    drop(upload); // the backend retires its prepared revision and drops its bytes
    // A revision that was never taken, or an unknown one, is rejected too.
    assert_eq!(service.try_commit(key, 2), CommitOutcome::Rejected);
    assert_eq!(service.try_commit(key, 99), CommitOutcome::Rejected);
    teardown(&service);
}

/// T11: a request admitted after the commit is a later replacement; the committed revision stays committed, becomes
/// shown on adoption, and the new one proceeds.
#[test]
fn t11_a_request_admitted_after_the_commit_leaves_it_committed_and_proceeds() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("revision 1");
    let first = uploads(&service).pop().expect("revision 1 taken");
    assert_eq!(service.try_commit(key, 1), CommitOutcome::Committed);
    assert_eq!(replace(&service, key), 2);
    let report = status(&service, key);
    assert_eq!(
        (report.committed, report.requested),
        (Some(1), 2),
        "1 stays committed"
    );
    service.adopted(key, 1);
    let report = status(&service, key);
    assert_eq!(
        (report.state, report.shown.map(|s| s.revision)),
        (StatusState::Ready, Some(1))
    );
    step(&service).expect("revision 2 proceeds");
    let second = uploads(&service).pop().expect("revision 2 taken");
    assert_eq!(service.try_commit(key, 2), CommitOutcome::Committed);
    service.adopted(key, 2);
    assert_eq!(status(&service, key).shown.map(|s| s.revision), Some(2));
    drop((first, second));
    teardown(&service);
}

/// T12: an acknowledgement for anything but the current committed revision is ignored.
#[test]
fn t12_an_acknowledgement_for_the_wrong_revision_is_ignored() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("revision 1");
    let upload = uploads(&service).pop().expect("taken");
    service.adopted(key, 1); // not committed yet
    assert_eq!(status(&service, key).shown, None);
    let _ = service.try_commit(key, 1);
    service.adopted(key, 7); // another revision
    assert_eq!(status(&service, key).shown, None);
    service.adopted(key, 1);
    assert_eq!(status(&service, key).shown.map(|s| s.revision), Some(1));
    service.adopted(key, 1); // again: nothing changes
    assert_eq!(status(&service, key).state, StatusState::Ready);
    drop(upload);
    teardown(&service);
}

/// T13: a failed upload of the current revision is recorded like a failed job; another revision's is ignored.
#[test]
fn t13_a_failed_upload_keeps_the_last_good_content_and_a_later_success_clears_the_error() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("revision 1");
    let first = uploads(&service).pop().expect("taken");
    let _ = service.try_commit(key, 1);
    service.adopted(key, 1);
    replace(&service, key);
    step(&service).expect("revision 2");
    let second = uploads(&service).pop().expect("revision 2 taken");
    service.upload_failed(key, 1, "not the current revision");
    assert_eq!(status(&service, key).error, None, "ignored");
    service.upload_failed(key, 2, "over the texture budget");
    drop(second); // the backend drops its Arc
    let report = status(&service, key);
    assert_eq!(report.state, StatusState::Ready, "revision 1 stays shown");
    assert_eq!(report.shown.map(|s| s.revision), Some(1));
    let error = report.error.expect("the error");
    assert_eq!(
        (error.revision, error.message()),
        (2, "over the texture budget")
    );
    assert_eq!(
        service.try_commit(key, 2),
        CommitOutcome::Rejected,
        "a failed upload does not commit"
    );
    // A later success clears it.
    replace(&service, key);
    step(&service).expect("revision 3");
    let third = uploads(&service).pop().expect("taken");
    let _ = service.try_commit(key, 3);
    service.adopted(key, 3);
    let report = status(&service, key);
    assert_eq!(
        (report.shown.map(|s| s.revision), report.error),
        (Some(3), None)
    );
    drop((first, third));
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Committed versus shown (F1)
// ---------------------------------------------------------------------------------------------

/// The stepped schedules of F1: an earlier shown revision (optional, 1x1), a committed one (3x2) awaiting its
/// acknowledgement, and a newer request that fails in its job or in `upload_failed`.
fn committed_then_failed(earlier_shown: bool, fails_in_upload: bool) {
    let service = service(Mode::Backend);
    let mut kept = Vec::new();
    let key = service
        .create_from_pixels(opaque(&service, 1, 1))
        .expect("admitted");
    let mut revision = 1;
    if earlier_shown {
        step(&service).expect("the earlier revision");
        kept.extend(backend_adopts(&service));
        revision += 1;
        service
            .replace_pixels(key, opaque(&service, 3, 2))
            .expect("admitted");
    }
    let committed = revision;
    // The candidate: finished, taken and committed, not yet acknowledged.
    step(&service).expect("the candidate");
    let candidate = uploads(&service).pop().expect("taken");
    assert_eq!(
        (
            candidate.revision,
            (candidate.pixels.width(), candidate.pixels.height())
        ),
        (committed, (if earlier_shown { (3, 2) } else { (1, 1) }))
    );
    assert_eq!(service.try_commit(key, committed), CommitOutcome::Committed);
    // The newer request fails.
    let failing = committed + 1;
    let reservation = service.reserve_pixels(4, 4).expect("reserved");
    let bad = CpuPixels::premultiplied(reservation, [9, 0, 0, 1].repeat(16)).expect("length");
    assert_eq!(service.replace_pixels(key, bad).expect("admitted"), failing);
    if fails_in_upload {
        // The job succeeds (valid pixels), the backend takes it and the upload fails.
        let reservation = service.reserve_pixels(4, 4).expect("reserved");
        let good = CpuPixels::premultiplied(reservation, vec![0; 64]).expect("length");
        service.replace_pixels(key, good).expect("admitted");
        step(&service).expect("the job of the replacement");
        let taken = uploads(&service).pop().expect("taken");
        service.upload_failed(key, taken.revision, "too big for the texture budget");
        kept.push(taken.pixels);
    } else {
        step(&service).expect("the failing job");
    }
    let report = status(&service, key);
    let error = report.error.expect("the newer request's error");
    assert!(error.revision > committed);
    assert_eq!(report.committed, Some(committed));
    if earlier_shown {
        // Before the acknowledgement: ready, and the shown size is the earlier revision's.
        assert_eq!(report.state, StatusState::Ready);
        assert_eq!(
            report.shown,
            Some(ShownRevision {
                revision: 1,
                width: 1,
                height: 1
            })
        );
    } else {
        assert_eq!(
            report.state,
            StatusState::Uploading,
            "no earlier shown revision, so uploading"
        );
        assert_eq!(report.shown, None);
    }
    // The acknowledgement: now the committed revision is shown; the error is newer, so it stays.
    service.adopted(key, committed);
    let report = status(&service, key);
    assert_eq!(report.state, StatusState::Ready);
    let (width, height) = if earlier_shown { (3, 2) } else { (1, 1) };
    assert_eq!(
        report.shown,
        Some(ShownRevision {
            revision: committed,
            width,
            height
        })
    );
    assert_eq!(
        report.error.as_ref().map(|e| e.revision),
        Some(error.revision)
    );
    drop((candidate, kept));
    teardown(&service);
}

/// T7, T12, F1: the review's worked case and its variants.
#[test]
fn f1_a_replacement_failing_in_its_job_after_a_commit_never_manufactures_an_adoption() {
    committed_then_failed(false, false);
    committed_then_failed(true, false);
}

/// T13, T12, F1.
#[test]
fn f1_a_replacement_failing_in_upload_failed_after_a_commit_never_manufactures_an_adoption() {
    committed_then_failed(false, true);
    committed_then_failed(true, true);
}

// ---------------------------------------------------------------------------------------------
// Linearisation with ownership loss (F4)
// ---------------------------------------------------------------------------------------------

/// A resource whose public token is released while a reference keeps it live; returns the key and the reference.
fn kept_by_a_quad(service: &ResourceService) -> (TextureKey, TextureRef<ResourceState>) {
    let key = create(service);
    let reference = service.retain(key).expect("retained");
    service.release(key).expect("released");
    (key, reference)
}

/// F4 (b): the public token released while a reference exists: every acceptance still succeeds.
#[test]
fn f4_b_a_released_token_with_a_live_reference_still_accepts_results_commits_and_adoptions() {
    let service = service(Mode::Backend);
    let (key, reference) = kept_by_a_quad(&service);
    step(&service).expect("the job");
    let upload = uploads(&service)
        .pop()
        .expect("accepted: waiting for upload");
    assert_eq!(service.try_commit(key, 1), CommitOutcome::Committed);
    service.adopted(key, 1);
    let report = service.status_of(&reference);
    assert_eq!(
        (report.state, report.shown.map(|s| s.revision)),
        (StatusState::Ready, Some(1))
    );
    drop((upload, reference));
    teardown(&service);
}

/// F4 (a): the last reference dropped, the registry's FIFO not yet serviced by `maintain`: a current completion, a
/// commit and an adoption are each discarded or rejected, and nothing becomes committed or shown.
#[test]
fn f4_a_work_arriving_after_the_last_reference_dropped_is_rejected_before_maintain_runs() {
    let service = service(Mode::Backend);

    // A current completion.
    let (completing, reference) = kept_by_a_quad(&service);
    let job = service.take_job().expect("executing");
    let output = job.run();
    drop(reference); // retiring now; nothing has serviced it
    service.complete(output);
    let report = status(&service, completing);
    assert_eq!((report.shown, report.committed), (None, None));
    assert!(
        uploads(&service).is_empty(),
        "the result was discarded, not offered"
    );
    assert_eq!(reserved(&service)[1], 0, "its bytes came back");

    // A commit.
    let (committing, reference) = kept_by_a_quad(&service);
    step(&service).expect("the job");
    let upload = uploads(&service).pop().expect("taken");
    drop(reference);
    assert_eq!(service.try_commit(committing, 1), CommitOutcome::Rejected);
    assert_eq!(status(&service, committing).committed, None);

    // An adoption.
    let (adopting, reference) = kept_by_a_quad(&service);
    step(&service).expect("the job");
    let second = uploads(&service).pop().expect("taken");
    assert_eq!(service.try_commit(adopting, 1), CommitOutcome::Committed);
    drop(reference);
    service.adopted(adopting, 1);
    assert_eq!(
        status(&service, adopting).shown,
        None,
        "an adoption on a retiring entry is ignored"
    );

    drop((upload, second));
    // maintain then cleans all three up.
    retire_until(&service, 3);
    teardown(&service);
}

/// F4 (c): the last reference dropped on another thread while the gate holds an acceptance inside its critical
/// section: the drop completes only after the decision, and the outcome is the linear order "accepted, then retired".
#[test]
fn f4_c_a_last_drop_racing_a_commit_waits_for_the_decision_and_is_then_cleaned_up() {
    let service = service(Mode::Backend);
    let (key, reference) = kept_by_a_quad(&service);
    step(&service).expect("the job");
    let upload = uploads(&service).pop().expect("taken");

    let (inside_tx, inside) = mpsc::channel::<()>();
    let (release, release_rx) = mpsc::channel::<()>();
    let state = Mutex::new((inside_tx, release_rx));
    service.set_gate(Some(Arc::new(move || {
        let state = state.lock().unwrap();
        let _ = state.0.send(());
        let _ = state.1.recv_timeout(BOUND);
    })));

    let committer = {
        let service = service.clone();
        thread::spawn(move || service.try_commit(key, 1))
    };
    inside
        .recv_timeout(BOUND)
        .expect("the commit is inside its critical section");
    service.set_gate(None);
    let finished = Arc::new(AtomicUsize::new(0));
    let last_drop = {
        let dropped = Arc::clone(&finished);
        thread::spawn(move || {
            drop(reference);
            dropped.store(1, Ordering::SeqCst);
        })
    };
    // The drop needs the registry lock, which the commit holds: it cannot finish before the decision.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the last drop overtook the decision"
    );
    release.send(()).expect("let the commit go on");
    assert_eq!(committer.join().expect("joined"), CommitOutcome::Committed);
    last_drop.join().expect("joined");
    // Accepted, then the entry retired: committed content on a retiring entry, which maintain cleans up.
    assert_eq!(service.registry().stats().retiring, 1);
    assert_eq!(status(&service, key).committed, Some(1));
    drop(upload);
    retire_until(&service, 1);
    assert_eq!(service.status(key), Err(HandleError::Stale(key)));
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Retirement: T14, T15, T16
// ---------------------------------------------------------------------------------------------

/// T14, T15, T16: after the release, the last `TextureRef` is dropped on another thread; the retiring wake fires;
/// `maintain` and the backend's `retired` then free the slot, with no create, replace, status or release in between.
#[test]
fn t14_t15_t16_retirement_needs_no_later_caller_action() {
    let calls = Calls::default();
    let service = calls.service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("the job");
    let kept = backend_adopts(&service);
    let reference = service.retain(key).expect("retained");
    service.release(key).expect("released");
    assert_eq!(calls.counts()[2], 0, "a reference still keeps it live");
    let dropper = thread::spawn(move || drop(reference));
    wait_until("the retiring wake", || calls.counts()[2] >= 1);
    dropper.join().expect("joined");
    assert!(service.retirement_pending());
    // T15: maintain starts the clean-up and asks the backend to retire.
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 1,
            finished: 0
        }
    );
    assert!(!service.retirement_pending());
    let mut asked = Vec::new();
    assert_eq!(service.take_retirements(&mut asked, 8), 1);
    assert_eq!(asked, [key]);
    // Still not finished: the backend has not proved nothing remains in use.
    assert_eq!(service.maintain(8), Maintained::default());
    drop(kept);
    // T16: `retired` completes it; the slot is reusable at the next generation.
    service.retired(key);
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 0,
            finished: 1
        }
    );
    let next = create(&service);
    assert_eq!(
        (next.slot(), next.generation()),
        (key.slot(), key.generation() + 1)
    );
    assert_eq!(service.status(key), Err(HandleError::Stale(key)));
    teardown(&service);
}

/// T15: maintain drops the queued request and a revision waiting for upload (returning their bytes) and marks an
/// executing job discard-only; the resource waits for that job's `complete`.
#[test]
fn t15_maintain_drops_queued_and_waiting_work_and_waits_for_an_executing_job() {
    let service = service(Mode::Headless);
    // A queued request.
    let queued = create(&service);
    // An executing job with a queued successor.
    let busy = create(&service);
    let first = loop {
        let job = service.take_job().expect("a job");
        if job.key() == busy {
            break job;
        }
        service.complete(job.run());
    };
    replace(&service, busy);
    assert!(reserved(&service)[1] >= 32);
    service.release(queued).expect("released");
    service.release(busy).expect("released");
    let cpu_before = reserved(&service)[1];
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 2,
            finished: 1
        }
    );
    // `queued` finished at once (headless: no backend); `busy` waits for its job. Its queued successor was dropped.
    assert!(
        reserved(&service)[1] < cpu_before,
        "the queued sources' bytes came back"
    );
    assert_eq!(service.status(queued), Err(HandleError::Stale(queued)));
    assert_eq!(status(&service, busy).requested, 2);
    service.complete(first.run());
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 0,
            finished: 1
        }
    );
    assert_eq!(service.registry().stats().retiring, 0);
    teardown(&service);
}

/// T15 in `Backend` mode: a revision waiting for upload is dropped at once.
#[test]
fn t15_a_revision_waiting_for_upload_is_dropped_when_the_resource_retires() {
    let service = service(Mode::Backend);
    let key = create(&service);
    step(&service).expect("the job");
    assert_eq!(reserved(&service)[1], 16);
    service.release(key).expect("released");
    let _ = service.maintain(8);
    assert_eq!(
        reserved(&service)[1],
        0,
        "the waiting revision's bytes returned"
    );
    assert!(uploads(&service).is_empty());
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Detach, close: T17, T18
// ---------------------------------------------------------------------------------------------

/// T17: after `detach_backend` no upload is offered, waiting revisions are dropped, a pending commit is rejected,
/// results are discarded, and retirement no longer waits for `retired`.
#[test]
fn t17_detach_stops_uploads_rejects_commits_discards_results_and_retirement_stops_waiting() {
    let service = service(Mode::Backend);
    let taken = create(&service);
    let _waiting = create(&service);
    let running = create(&service);
    let retiring = create(&service);
    step(&service).expect("taken's job");
    step(&service).expect("waiting's job");
    let mut out = Vec::new();
    service.take_uploads(&mut out, 1); // `taken` is with the backend
    assert_eq!(out[0].key, taken);
    let job = service.take_job().expect("running's job");
    assert_eq!(job.key(), running);
    service.release(retiring).expect("released");
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 1,
            finished: 0
        },
        "waits for the backend"
    );
    let cpu_before = reserved(&service)[1];

    service.detach_backend();
    assert_eq!(
        reserved(&service)[1],
        cpu_before - 16,
        "the waiting revision was dropped"
    );
    assert_eq!(service.take_uploads(&mut out, 8), 0, "no upload is offered");
    assert_eq!(
        service.try_commit(taken, 1),
        CommitOutcome::Rejected,
        "a pending commit is rejected"
    );
    service.complete(job.run());
    assert_eq!(
        status(&service, running).state,
        StatusState::Pending,
        "the late result was discarded"
    );
    // Retirement finishes without `retired`.
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 0,
            finished: 1
        }
    );
    service.detach_backend(); // idempotent
    drop(out);
    teardown(&service);
}

/// T18: `close` moves the queued and waiting work out and drops it after the locks, so their reservations are back when
/// it returns; the rest reaches zero through `complete`, the backend's drops, `maintain` and `finish_retirement`.
#[test]
fn t18_close_returns_the_queued_and_waiting_reservations_before_it_returns() {
    let service = service(Mode::Backend);
    // 4 queued; 3 waiting for upload; 2 with the backend; 2 executing.
    let queued: Vec<_> = (0..4).map(|_| create(&service)).collect();
    let _ = &queued;
    let executing: Vec<_> = (0..2).map(|_| service.take_job().expect("a job")).collect();
    let outputs: Vec<_> = executing.into_iter().map(Job::run).collect();
    let mut held = Vec::new();
    for _ in 0..5 {
        create(&service);
    }
    // Finish five of them, take two of those.
    for _ in 0..5 {
        step(&service).expect("a job");
    }
    service.take_uploads(&mut held, 2);
    let before = reserved(&service);
    // queued: 4+... the exact numbers are not the point: after close, only what is executing or held remains.
    service.close();
    let after = reserved(&service);
    assert!(
        after[0] < before[0] && after[1] < before[1],
        "{before:?} -> {after:?}"
    );
    // What remains: the two executing jobs (job slots, their sources) and the two Arcs the backend holds.
    assert_eq!(after[0], 2, "only the executing jobs hold job slots");
    assert_eq!(
        after[1],
        2 * 16 + 2 * 16,
        "only the executing sources and the backend's revisions hold bytes"
    );
    for output in outputs {
        service.complete(output); // discarded: the service is closed
    }
    assert_eq!(reserved(&service)[0], 0);
    assert_eq!(reserved(&service)[1], 2 * 16);
    drop(held);
    assert!(service.take_job().is_none());
    let refused = service
        .create_from_pixels(opaque(&service, 2, 2))
        .expect_err("ended");
    assert_eq!(refused.error, AdmitError::Ended);
    drop(refused);
    service.close(); // a second close does nothing
    teardown(&service);
}

#[test]
fn close_ends_public_ownership_everywhere_and_retirement_still_finishes() {
    let service = service(Mode::Headless);
    let plain = create(&service);
    let kept = create(&service);
    let reference = service.retain(kept).expect("retained");
    service.close();
    assert_eq!(service.check(plain), Err(HandleError::Closed));
    assert_eq!(service.retain(plain).unwrap_err(), HandleError::Closed);
    assert_eq!(service.release(plain), Err(HandleError::Closed));
    assert!(
        service.status(plain).is_ok(),
        "the owner's snapshot keeps working"
    );
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 1,
            finished: 1
        }
    );
    drop(reference);
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 1,
            finished: 1
        }
    );
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Wakes, locks, order and bounds
// ---------------------------------------------------------------------------------------------

#[test]
fn the_wakes_fire_when_a_queue_goes_non_empty_with_both_locks_free() {
    let calls = Calls::default();
    let service = calls.service(Mode::Backend);
    let first = create(&service);
    assert_eq!(calls.counts(), [1, 0, 0], "the job queue went non-empty");
    let second = create(&service);
    assert_eq!(
        calls.counts(),
        [1, 0, 0],
        "it was already non-empty: no second wake"
    );
    step(&service).expect("first");
    step(&service).expect("second");
    assert_eq!(
        calls.counts()[1],
        1,
        "one wake for the two revisions waiting"
    );
    // Draining the queue empties it, so the next arrival wakes again.
    let kept = backend_adopts(&service);
    replace(&service, first);
    assert_eq!(calls.counts()[0], 2);
    // The retirement wake: release of an unreferenced resource.
    service.release(second).expect("released");
    assert_eq!(calls.counts()[2], 1);
    let _ = service.maintain(8);
    let mut keys = Vec::new();
    service.take_retirements(&mut keys, 8);
    drop(kept);
    for key in keys {
        service.retired(key);
    }
    assert_eq!(
        calls.under_lock.load(Ordering::SeqCst),
        0,
        "a wake ran with a lock held"
    );
    teardown(&service);
}

/// The drop probe: no pixel buffer, source, revision or resource state is dropped with a lock held, across T2, T6, T15,
/// T16, T17 and T18 (the other tests assert the same at their end).
#[test]
fn no_value_is_dropped_with_a_lock_held_across_the_transitions_that_drop_values() {
    let before = lockdebug::violations();
    let service = service(Mode::Backend);
    // T2: a replaced queued request.
    let key = create(&service);
    replace(&service, key);
    // T6: a superseded and a retiring result.
    let job = service.take_job().expect("a job");
    let output = job.run();
    replace(&service, key);
    service.complete(output);
    // T15 and T16.
    service.release(key).expect("released");
    let _ = service.maintain(8);
    let mut keys = Vec::new();
    service.take_retirements(&mut keys, 8);
    for key in keys {
        service.retired(key);
    }
    let _ = service.maintain(8);
    // T17 and T18.
    let other = create(&service);
    step(&service).expect("a job");
    service.detach_backend();
    let _ = other;
    teardown(&service);
    assert_eq!(lockdebug::violations(), before);
}

#[test]
fn a_service_built_with_a_bad_limit_or_an_exhausted_ledger_identity_is_refused() {
    let error = ResourceService::new(limits(0, 1), Mode::Headless).unwrap_err();
    assert!(
        matches!(
            error,
            ServiceError::Limits(LimitsError {
                field: "max_textures",
                ..
            })
        ),
        "{error:?}"
    );
    let exhausted = ResourceService::build(
        ResourceLimits::DEFAULT,
        Mode::Headless,
        ServiceWakes::default(),
        None,
        Err(LedgerIdsExhausted),
    )
    .unwrap_err();
    assert_eq!(exhausted, ServiceError::Ledger(LedgerIdsExhausted));
}

/// A long scripted churn with tiny limits: the queues and the budgets never exceed their bounds, and everything drains.
#[test]
fn order_and_bounds_hold_under_a_long_scripted_churn() {
    let service = small(4, 3, Mode::Backend);
    let mut keys: Vec<TextureKey> = Vec::new();
    let mut held: Vec<UploadRequest> = Vec::new();
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..2000 {
        match next() % 8 {
            0 => {
                if let Ok(key) = service.create_from_pixels(opaque(&service, 1, 1)) {
                    keys.push(key);
                }
            }
            1 | 2 => {
                if let Some(key) =
                    keys.get(usize::try_from(next()).unwrap_or(0) % keys.len().max(1))
                {
                    let _ = service.replace_pixels(*key, opaque(&service, 1, 1));
                }
            }
            3 => {
                if !keys.is_empty() {
                    let index = usize::try_from(next()).unwrap_or(0) % keys.len();
                    let key = keys.swap_remove(index);
                    let _ = service.release(key);
                }
            }
            4 | 5 => {
                let _ = step(&service);
            }
            6 => {
                let mut out = Vec::new();
                service.take_uploads(&mut out, 2);
                for upload in out {
                    if service.try_commit(upload.key, upload.revision) == CommitOutcome::Committed {
                        service.adopted(upload.key, upload.revision);
                    }
                    held.push(upload);
                }
                if held.len() > 3 {
                    held.drain(..2);
                }
            }
            _ => {
                let _ = service.maintain(4);
                let mut asked = Vec::new();
                service.take_retirements(&mut asked, 4);
                for key in asked {
                    service.retired(key);
                }
            }
        }
        let stats = service.stats();
        assert!(stats.jobs.reserved <= 3, "round {round}: {stats:?}");
        assert!(service.queue_lengths()[0] <= 3 && service.queue_lengths()[1] <= 4);
        let registry = stats.registry;
        assert_eq!(
            registry.live + registry.retiring + registry.exhausted + registry.free,
            4,
            "round {round}"
        );
        assert!(registry.live <= 4);
    }
    drop(held);
    teardown(&service);
}

/// Jobs and uploads come out in the order they were admitted.
#[test]
fn jobs_and_uploads_come_out_fifo() {
    let service = service(Mode::Backend);
    let keys: Vec<_> = (0..5).map(|_| create(&service)).collect();
    let mut order = Vec::new();
    while let Some((key, _)) = step(&service) {
        order.push(key);
    }
    assert_eq!(order, keys);
    let taken: Vec<_> = uploads(&service).iter().map(|u| u.key).collect();
    assert_eq!(taken, keys);
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// Review 1 (CK-107): F1 retirement continuation, F2 upload order, F3 bounded steps
// ---------------------------------------------------------------------------------------------

/// A wake-driven owner: it calls `maintain(limit)` once per retirement wake or pending flag it has not yet answered,
/// and does nothing else (no release, proof, completion or other caller edit). Returns how many steps it took; fails
/// the test if it would spin.
fn owner_driven_by_the_protocol(service: &ResourceService, calls: &Calls, limit: usize) -> usize {
    let mut seen = calls.counts()[2];
    let mut steps = 0;
    while service.retirement_pending() || calls.counts()[2] != seen {
        seen = calls.counts()[2];
        let _ = service.maintain(limit);
        steps += 1;
        assert!(steps <= 20, "the owner would spin: {steps} steps");
    }
    steps
}

/// F1: B's backend proof arrives behind A's, which is still outstanding; with a step of one resource, the owner driven
/// only by the wake and the pending flag still finishes B, and then stops asking.
#[test]
fn f1_a_ready_retirement_behind_a_blocked_one_is_finished_by_the_wake_driven_owner() {
    let calls = Calls::default();
    let service = calls.service(Mode::Backend);
    let (a, b) = (create(&service), create(&service));
    service.release(a).expect("released");
    service.release(b).expect("released");
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 2,
            finished: 0
        }
    );
    let mut asked = Vec::new();
    service.take_retirements(&mut asked, 8);
    assert_eq!(asked, [a, b]);
    // Nothing is pending: a complete scan found both blocked.
    let _ = owner_driven_by_the_protocol(&service, &calls, 1);
    assert!(!service.retirement_pending());
    // B's proof arrives; A's does not.
    service.retired(b);
    assert!(service.retirement_pending());
    let steps = owner_driven_by_the_protocol(&service, &calls, 1);
    assert_eq!(steps, 2, "A (blocked) first, then B");
    assert_eq!(service.status(b), Err(HandleError::Stale(b)), "B was freed");
    assert!(
        service.status(a).is_ok(),
        "A is still waiting for its proof"
    );
    assert!(
        !service.retirement_pending(),
        "only blocked work is left: no more asking"
    );
    assert_eq!(owner_driven_by_the_protocol(&service, &calls, 1), 0);
    service.retired(a);
    let _ = owner_driven_by_the_protocol(&service, &calls, 1);
    assert_eq!(service.status(a), Err(HandleError::Stale(a)));
    teardown(&service);
}

/// F1, the completion-woken case (no backend): B's job ends behind A's, which still runs.
#[test]
fn f1_a_retirement_woken_by_a_completion_is_finished_behind_a_blocked_one() {
    let calls = Calls::default();
    let service = calls.service(Mode::Headless);
    let (a, b) = (create(&service), create(&service));
    let first = service.take_job().expect("a's job");
    let second = service.take_job().expect("b's job");
    assert_eq!((first.key(), second.key()), (a, b));
    service.release(a).expect("released");
    service.release(b).expect("released");
    assert_eq!(
        service.maintain(8),
        Maintained {
            started: 2,
            finished: 0
        }
    );
    let _ = owner_driven_by_the_protocol(&service, &calls, 1);
    assert!(!service.retirement_pending());
    // B's job ends (its result is discarded): the completion wakes the owner.
    service.complete(second.run());
    assert!(service.retirement_pending());
    let steps = owner_driven_by_the_protocol(&service, &calls, 1);
    assert_eq!(steps, 2);
    assert_eq!(service.status(b), Err(HandleError::Stale(b)));
    assert!(service.status(a).is_ok());
    service.complete(first.run());
    let _ = owner_driven_by_the_protocol(&service, &calls, 1);
    assert_eq!(service.status(a), Err(HandleError::Stale(a)));
    teardown(&service);
}

/// F1: many blocked resources and a step of one: the owner takes a bounded number of steps and then stops.
#[test]
fn f1_a_scan_that_finds_only_blocked_work_stops_asking_to_be_called() {
    let calls = Calls::default();
    let service = calls.service(Mode::Backend);
    let keys: Vec<_> = (0..6).map(|_| create(&service)).collect();
    for key in &keys {
        service.release(*key).expect("released");
    }
    let steps = owner_driven_by_the_protocol(&service, &calls, 1);
    assert!(steps <= keys.len() * 2 + 2, "{steps}");
    assert!(!service.retirement_pending());
    assert_eq!(
        service.registry().stats().retiring,
        6,
        "all still blocked on the backend"
    );
    let mut asked = Vec::new();
    service.take_retirements(&mut asked, 16);
    for key in asked {
        service.retired(key);
        let _ = owner_driven_by_the_protocol(&service, &calls, 1);
    }
    assert_eq!(service.registry().stats().retiring, 0);
    teardown(&service);
}

/// F2: A1 and B1 finish, A is replaced and A2 finishes before the backend pulls: B1 has waited longer than A2.
#[test]
fn f2_a_replacement_takes_its_own_place_in_upload_order_behind_the_older_revision_of_another_resource()
 {
    let service = service(Mode::Backend);
    let (a, b) = (create(&service), create(&service));
    step(&service).expect("a1");
    step(&service).expect("b1");
    assert_eq!(service.queued_keys().1, [a, b]);
    replace(&service, a); // a2: a1's waiting revision is dropped, and its position goes with it
    assert_eq!(
        service.queued_keys().1,
        [b],
        "the superseded revision left the queue"
    );
    step(&service).expect("a2");
    assert_eq!(service.queued_keys().1, [b, a], "a2 queues behind b1");
    let mut out = Vec::new();
    assert_eq!(service.take_uploads(&mut out, 1), 1);
    assert_eq!((out[0].key, out[0].revision), (b, 1), "b1 waited longer");
    assert_eq!(service.take_uploads(&mut out, 1), 1);
    assert_eq!((out[1].key, out[1].revision), (a, 2));
    drop(out);
    teardown(&service);
}

/// F2: a replacement that fails removes the stale position and leaves no upload for the resource.
#[test]
fn f2_a_failed_replacement_leaves_no_stale_upload_position() {
    let service = service(Mode::Backend);
    let (a, b) = (create(&service), create(&service));
    step(&service).expect("a1");
    step(&service).expect("b1");
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bad = CpuPixels::premultiplied(reservation, [9, 0, 0, 1].repeat(4)).expect("length");
    service.replace_pixels(a, bad).expect("admitted");
    assert_eq!(service.queued_keys().1, [b]);
    step(&service).expect("a2 fails");
    assert_eq!(
        service.queued_keys().1,
        [b],
        "no stale position for the failed replacement"
    );
    assert_eq!(status(&service, a).state, StatusState::Failed);
    let offered: Vec<_> = uploads(&service)
        .iter()
        .map(|u| (u.key, u.revision))
        .collect();
    assert_eq!(offered, [(b, 1)]);
    teardown(&service);
}

/// F3: retiring one resource does the same bounded work whatever the backlog of unrelated queued resources.
#[test]
fn f3_maintain_retiring_one_resource_does_work_independent_of_the_backlog() {
    let mut costs = Vec::new();
    for backlog in [10_u32, 400, 3000] {
        let service = small(4096, 4096, Mode::Headless);
        let keys: Vec<_> = (0..backlog)
            .map(|_| {
                service
                    .create_from_pixels(opaque(&service, 1, 1))
                    .expect("admitted")
            })
            .collect();
        // Half have finished jobs (waiting would need a backend); the rest stay queued. The one to retire is in the middle.
        let target = keys[keys.len() / 2];
        let before = service.work_done();
        service.release(target).expect("released");
        let done = service.maintain(1);
        assert_eq!(done.started, 1);
        let after = service.work_done();
        costs.push((after.0 - before.0, after.1 - before.1));
        drop(keys);
        teardown(&service);
    }
    assert!(costs.windows(2).all(|pair| pair[0] == pair[1]), "{costs:?}");
    assert!(
        costs[0].1 <= 12,
        "a constant number of queue nodes: {costs:?}"
    );
}

/// F3: a bounded pull looks at a bounded number of queued resources, even when many are retiring and not yet cleaned up;
/// when its budget ends with work left it asks to be called again.
#[test]
fn f3_pulls_look_at_a_bounded_number_of_resources_and_ask_again_when_work_is_left() {
    let calls = Calls::default();
    let service = calls.service(Mode::Backend);
    let keys: Vec<_> = (0..120).map(|_| create(&service)).collect();
    while step(&service).is_some() {}
    assert_eq!(service.queued_keys().1.len(), 120);
    // The first 100 are retiring and `maintain` has not run: their positions are in the upload queue still.
    for key in &keys[..100] {
        service.release(*key).expect("released");
    }
    let mut taken = 0;
    let mut calls_made = 0;
    let mut out = Vec::new();
    while taken < 20 {
        let before = service.work_done().0;
        let wakes_before = calls.counts()[1];
        taken += service.take_uploads(&mut out, 1);
        calls_made += 1;
        let looked_at = service.work_done().0 - before;
        assert!(looked_at <= 1 + 8, "one pull looked at {looked_at}");
        if looked_at == 1 + 8 && taken < 20 {
            assert!(
                calls.counts()[1] > wakes_before,
                "a pull that ran out of budget asks to be called again"
            );
        }
        assert!(calls_made < 60, "the pulls do not make progress");
    }
    assert_eq!(out.iter().map(|u| u.key).collect::<Vec<_>>(), keys[100..]);
    drop(out);
    // The same for jobs: 100 more queued, the first 90 retiring and not cleaned up.
    let more: Vec<_> = (0..100).map(|_| create(&service)).collect();
    for key in &more[..90] {
        service.release(*key).expect("released");
    }
    let mut found = None;
    let mut tries = 0;
    while found.is_none() {
        let before = service.work_done().0;
        found = service.take_job();
        tries += 1;
        assert!(
            service.work_done().0 - before <= 8,
            "a job step looks at no more than 8 resources"
        );
        assert!(tries <= 20, "take_job does not make progress");
    }
    let job = found.expect("a job");
    assert_eq!(job.key(), more[90]);
    service.complete(job.run());
    teardown(&service);
}

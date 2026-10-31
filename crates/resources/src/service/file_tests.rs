//! CK-108: file sources in the service (`load`, `reload`), driven by hand where a test needs an exact interleaving and by the
//! loader pool where it needs real threads. Every test ends by closing the service, retiring everything and asserting all
//! three budgets are back to zero reserved and that no value was dropped with a lock held. No test sleeps for timing: it
//! waits on observable state with a bounded deadline and holds threads with latches at the points of the protocols.

use std::fs;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;
use crate::testkit::{
    Backend, Scratch, file_service, limits, reserved, state, status, step, teardown, write_png,
};

const BOUND: Duration = Duration::from_secs(10);

fn opaque(service: &ResourceService, width: u32, height: u32) -> CpuPixels {
    let reservation = service
        .reserve_pixels(width, height)
        .expect("reserved pixels");
    let bytes = vec![255; reservation.byte_len()];
    CpuPixels::straight_alpha(reservation, bytes).expect("the right length")
}

fn uploads(service: &ResourceService) -> Vec<UploadRequest> {
    let mut out = Vec::new();
    service.take_uploads(&mut out, usize::MAX);
    out
}

/// A service over a root that holds the files a test writes.
fn headless(root: &Scratch) -> ResourceService {
    file_service(root.path(), Mode::Headless, limits(1, 64 * 1024 * 1024))
}

fn backend(root: &Scratch) -> ResourceService {
    file_service(root.path(), Mode::Backend, limits(1, 64 * 1024 * 1024))
}

/// The legacy decode is synchronous: it reserves CPU bytes before allocation, creates no job, and queues revision 1
/// directly for upload. Negative control: routing this through `load` leaves a job slot and a pending resource.
#[test]
fn create_decoded_admits_revision_one_without_a_loader_job() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "compat.png", 2, 3, [7, 11, 13, 255]);
    let service = backend(&scratch);
    let decoded = service
        .create_decoded("compat.png")
        .expect("admitted synchronously");
    assert_eq!((decoded.width, decoded.height), (2, 3));
    assert_eq!(service.stats().jobs.reserved, 0);
    assert_eq!(service.stats().cpu_pixels.reserved, 24);
    assert_eq!(service.stats().registry.live, 1);
    assert_eq!(status(&service, decoded.key).requested, 1);
    assert_eq!(status(&service, decoded.key).state, StatusState::Uploading);
    let uploads = uploads(&service);
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].revision, 1);
    assert_eq!(
        (uploads[0].pixels.width(), uploads[0].pixels.height()),
        (2, 3)
    );
    assert_eq!(
        uploads[0].pixels.image().pixels(),
        &[7, 11, 13, 255].repeat(6)
    );
    drop(uploads);
    teardown(&service);
}

/// File/decode and shared-budget failures do not leave a slot or reservation behind. Budget refusals remain distinct
/// from broken files. Negative control: allocating first would increase the CPU peak before reporting the budget.
#[test]
fn create_decoded_refusals_return_the_entire_admission() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "valid.png", 2, 2, [1, 2, 3, 255]);
    write_png(
        scratch.path(),
        "over-limit.png",
        MAX_SIDE + 1,
        1,
        [1, 2, 3, 255],
    );
    fs::write(scratch.path().join("garbage.png"), b"not a png").expect("garbage file");
    fs::write(
        scratch.path().join("truncated.png"),
        b"\x89PNG\r\n\x1a\n\0\0",
    )
    .expect("truncated file");
    let mut constrained = ResourceLimits::DEFAULT;
    constrained.cpu_pixel_bytes = 64 * 1024 * 1024;
    let service = ResourceService::with_assets(
        constrained,
        Mode::Backend,
        ServiceWakes::default(),
        Some(Assets::new(scratch.path())),
    )
    .expect("a constrained backend service");
    let before = (reserved(&service), service.stats().registry);
    for name in [
        "missing.png",
        "garbage.png",
        "truncated.png",
        "over-limit.png",
    ] {
        assert!(matches!(
            service.create_decoded(name),
            Err(LegacyAdmitError::Assets(_))
        ));
        assert_eq!(
            (reserved(&service), service.stats().registry),
            before,
            "{name}"
        );
    }

    let held = service
        .shared
        .ledger
        .reserve(BudgetKind::CpuPixelBytes, 64 * 1024 * 1024 - 8)
        .expect("leave only eight pixel bytes");
    let error = service
        .create_decoded("valid.png")
        .expect_err("16 bytes do not fit in the remaining budget");
    assert!(matches!(
        error,
        LegacyAdmitError::Budget {
            budget: LegacyBudget::CpuPixelBytes,
            requested: 16,
            available: 8,
        }
    ));
    assert_eq!(service.stats().cpu_pixels.reserved, 64 * 1024 * 1024 - 8);
    assert_eq!(service.stats().jobs.reserved, 0);
    assert_eq!(service.stats().registry.live, 0);
    drop(held);
    teardown(&service);
}

/// Registry and metadata limits held by other resources are reported as shared-budget refusals, with the decoded CPU
/// reservation returned and no extra registry entry. Negative control: checking capacity after creating state leaves a
/// charged slot behind.
#[test]
fn create_decoded_accounts_for_registry_and_metadata_capacity() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "valid.png", 1, 1, [1, 2, 3, 255]);

    let mut limits = ResourceLimits::DEFAULT;
    limits.max_textures = 1;
    limits.max_pending_jobs = 1;
    limits.cpu_pixel_bytes = 64 * 1024 * 1024;
    let slots = ResourceService::with_assets(
        limits,
        Mode::Backend,
        ServiceWakes::default(),
        Some(Assets::new(scratch.path())),
    )
    .expect("a one-slot service");
    let existing = slots
        .create_from_pixels(opaque(&slots, 1, 1))
        .expect("first resource");
    assert_eq!(step(&slots), Some((existing, 1)));
    let cpu_before = slots.stats().cpu_pixels.reserved;
    assert!(matches!(
        slots.create_decoded("valid.png"),
        Err(LegacyAdmitError::Budget {
            budget: LegacyBudget::RegistrySlots,
            requested: 1,
            available: 0,
        })
    ));
    assert_eq!(slots.stats().cpu_pixels.reserved, cpu_before);
    assert_eq!(slots.stats().registry.live, 1);
    teardown(&slots);

    let mut limits = ResourceLimits::DEFAULT;
    limits.max_textures = 1;
    limits.max_pending_jobs = 1;
    limits.cpu_pixel_bytes = 64 * 1024 * 1024;
    limits.metadata_bytes = 1024 * 1024;
    let metadata = ResourceService::with_assets(
        limits,
        Mode::Backend,
        ServiceWakes::default(),
        Some(Assets::new(scratch.path())),
    )
    .expect("a constrained metadata service");
    let remaining = RESOURCE_METADATA_BYTES - 1;
    let held = metadata
        .shared
        .ledger
        .reserve(BudgetKind::MetadataBytes, limits.metadata_bytes - remaining)
        .expect("hold all but one byte less than a resource needs");
    assert!(matches!(
        metadata.create_decoded("valid.png"),
        Err(LegacyAdmitError::Budget {
            budget: LegacyBudget::MetadataBytes,
            requested,
            available,
        }) if requested == RESOURCE_METADATA_BYTES && available == remaining
    ));
    assert_eq!(metadata.stats().cpu_pixels.reserved, 0);
    assert_eq!(metadata.stats().registry.live, 0);
    drop(held);
    teardown(&metadata);
}

/// A service closure racing the synchronous decode returns `Ended` and returns the pre-allocation reservation. The
/// gate is after reserve and before the decoder allocates its image. Negative control: admitting after close leaves a
/// live registry entry.
#[test]
fn create_decoded_rechecks_service_end_after_the_decode() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "compat.png", 2, 2, [7, 11, 13, 255]);
    let service = backend(&scratch);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    service.set_hook(Some(Arc::new(move |stage, _| {
        if stage == Stage::Reserved {
            entered_tx.send(()).expect("test is waiting");
            release_rx
                .lock()
                .unwrap()
                .recv()
                .expect("test releases decoder");
        }
    })));
    let decoding = service.clone();
    let worker = thread::spawn(move || decoding.create_decoded("compat.png"));
    entered_rx
        .recv_timeout(BOUND)
        .expect("decode reserved before allocating");
    service.close();
    release_tx.send(()).expect("release decoder");
    assert!(matches!(
        worker
            .join()
            .expect("decoder thread")
            .expect_err("closed service"),
        LegacyAdmitError::Ended
    ));
    assert_eq!(service.stats().cpu_pixels.reserved, 0);
    assert_eq!(service.stats().registry.live, 0);
    service.set_hook(None);
    teardown(&service);
}

/// `create_decoded` names the mode it refuses and checks an ended service before attempting to open a path.
#[test]
fn create_decoded_refuses_headless_and_previously_ended_services() {
    let scratch = Scratch::new();
    let headless = file_service(scratch.path(), Mode::Headless, limits(1, 64 * 1024 * 1024));
    assert!(matches!(
        headless.create_decoded("missing.png"),
        Err(LegacyAdmitError::Mode(Mode::Headless))
    ));
    teardown(&headless);

    let service = backend(&scratch);
    service.close();
    assert!(matches!(
        service.create_decoded("missing.png"),
        Err(LegacyAdmitError::Ended)
    ));
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// load: no file-system call on the caller's thread
// ---------------------------------------------------------------------------------------------

/// `load` makes no file-system call: with a root that does not exist yet it succeeds; creating the root and the file
/// before the job runs makes the resource ready. Negative control: a `load` that opened the file would fail here.
#[test]
fn load_makes_no_filesystem_call_and_the_job_finds_files_created_after_it() {
    let scratch = Scratch::new();
    let root = scratch.path().join("assets-made-later");
    let service = file_service(&root, Mode::Headless, limits(1, 64 * 1024 * 1024));
    let key = service
        .load("later.png")
        .expect("admitted with no root yet");
    assert_eq!(state(&service, key), StatusState::Pending);
    write_png(&root, "later.png", 3, 2, [10, 20, 30, 255]);
    assert_eq!(step(&service), Some((key, 1)));
    let now = status(&service, key);
    assert_eq!(now.state, StatusState::CpuReady, "{now:?}");
    assert_eq!(
        now.shown,
        Some(ShownRevision {
            revision: 1,
            width: 3,
            height: 2
        })
    );
    teardown(&service);
}

/// A file that is missing, a root that never exists, a link outside the root and a file that is not a PNG are asynchronous
/// errors in `status`, each with the message of the assets rule; none changes the shown content (there is none).
#[test]
fn file_errors_arrive_asynchronously_in_status() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "ok.png", 1, 1, [1, 2, 3, 255]);
    fs::write(scratch.path().join("garbage.png"), b"not a png at all").expect("a file");
    let service = headless(&scratch);
    let missing = service.load("missing.png").expect("admitted");
    let garbage = service.load("garbage.png").expect("admitted");
    let fine = service.load("ok.png").expect("admitted");
    while step(&service).is_some() {}
    let failed = status(&service, missing);
    assert_eq!(failed.state, StatusState::Failed);
    let error = failed.error.expect("an error");
    assert_eq!(error.revision, 1);
    assert!(
        error
            .message()
            .contains("cannot open image \"missing.png\""),
        "{}",
        error.message()
    );
    let failed = status(&service, garbage);
    assert_eq!(failed.state, StatusState::Failed);
    assert!(
        failed
            .error
            .expect("an error")
            .message()
            .contains("not a PNG"),
        "the decoder's own message"
    );
    assert_eq!(state(&service, fine), StatusState::CpuReady);
    teardown(&service);

    // A root that never exists: the first job reports it, and so does every later one.
    let never = file_service(
        &scratch.path().join("never"),
        Mode::Headless,
        limits(1, 64 * 1024 * 1024),
    );
    let one = never.load("a.png").expect("admitted");
    let two = never.load("b.png").expect("admitted");
    while step(&never).is_some() {}
    for key in [one, two] {
        let message = status(&never, key)
            .error
            .expect("an error")
            .message()
            .to_owned();
        assert!(message.contains("assets directory"), "{message}");
    }
    teardown(&never);
}

/// Every refusal of `load` is on the caller's thread and changes nothing: no resource, no budget, no queue entry.
#[test]
fn load_refusals_change_nothing() {
    let scratch = Scratch::new();
    let none = ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("a service");
    assert_eq!(none.load("a.png").unwrap_err(), AdmitError::NoAssets);

    let service = headless(&scratch);
    let long = "a".repeat(MAX_PATH_BYTES + 1);
    let cases: [(&str, &str); 6] = [
        ("", "it is empty"),
        (".", "it has a `.`"),
        ("a/./b.png", "it has a `.`"),
        ("../up.png", "it has a `..`"),
        ("/absolute.png", "it is not relative"),
        ("a/../b.png", "it has a `..`"),
    ];
    for (name, reason) in cases {
        match service.load(name) {
            Err(AdmitError::Name(message)) => {
                assert!(message.contains("is not allowed"), "{message}");
                assert!(message.contains(reason), "{name:?}: {message}");
                assert_eq!(
                    message,
                    Assets::check_name(name).unwrap_err().to_string(),
                    "the message is the assets rule's, unchanged"
                );
            }
            other => panic!("{name:?}: {other:?}"),
        }
    }
    assert_eq!(
        service.load(&long).unwrap_err(),
        AdmitError::PathTooLong {
            bytes: MAX_PATH_BYTES + 1
        }
    );
    // Exactly the limit is a name like any other.
    let exact = "a".repeat(MAX_PATH_BYTES);
    let key = service.load(&exact).expect("1024 bytes is allowed");
    assert_eq!(
        service.queue_lengths()[0],
        0,
        "a file request is not in the CPU queue"
    );
    let _ = service.release(key);
    assert_eq!(service.stats().registry.live, 0);
    while step(&service).is_some() {}
    teardown(&service);

    let fresh = headless(&scratch);
    let before = (reserved(&fresh), fresh.stats().registry);
    for name in ["", "../x.png", &long] {
        let _ = fresh.load(name);
    }
    assert_eq!((reserved(&fresh), fresh.stats().registry), before);
    teardown(&fresh);
}

/// A long name is charged to metadata and returned with the resource; two loads of one name are two resources.
#[test]
fn two_loads_of_one_name_are_two_resources_and_the_name_is_charged() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "same.png", 2, 2, [9, 9, 9, 255]);
    let service = headless(&scratch);
    let before = service.stats().metadata.reserved;
    let a = service.load("same.png").expect("admitted");
    let one = service.stats().metadata.reserved - before;
    let b = service.load("same.png").expect("admitted");
    assert_ne!(a, b);
    let two = service.stats().metadata.reserved - before;
    assert_eq!(two, 2 * one, "the same charge for the same name");
    assert!(
        one >= RESOURCE_METADATA_BYTES + JOB_METADATA_BYTES + "same.png".len() as u64,
        "the resource, the job and the name's length"
    );
    while step(&service).is_some() {}
    assert_eq!(state(&service, a), StatusState::CpuReady);
    assert_eq!(state(&service, b), StatusState::CpuReady);
    teardown(&service);
}

// ---------------------------------------------------------------------------------------------
// reload
// ---------------------------------------------------------------------------------------------

/// A reload decodes the file's new content as the next revision.
#[test]
fn a_reload_decodes_the_new_content_as_the_next_revision() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "pic.png", 2, 2, [255, 0, 0, 255]);
    let service = backend(&scratch);
    let key = service.load("pic.png").expect("admitted");
    let mut fake = Backend::default();
    assert_eq!(step(&service), Some((key, 1)));
    fake.take_all(&service);
    assert_eq!(fake_pixels(&fake, 0), [255, 0, 0, 255]);
    write_png(scratch.path(), "pic.png", 3, 1, [0, 255, 0, 255]);
    assert_eq!(service.reload(key), Ok(2));
    assert_eq!(step(&service), Some((key, 2)));
    fake.take_all(&service);
    assert_eq!(fake_pixels(&fake, 1), [0, 255, 0, 255]);
    let now = status(&service, key);
    assert_eq!(
        (now.state, now.requested, now.committed),
        (StatusState::Ready, 2, Some(2))
    );
    assert_eq!(
        now.shown.map(|shown| (shown.width, shown.height)),
        Some((3, 1))
    );
    fake.release_all();
    teardown(&service);
}

fn fake_pixels(fake: &Backend, index: usize) -> [u8; 4] {
    let revision = fake.revision(index);
    [
        revision.pixels()[0],
        revision.pixels()[1],
        revision.pixels()[2],
        revision.pixels()[3],
    ]
}

/// A reload whose file became broken keeps the old revision shown and committed, with the error beside it (§5.3).
#[test]
fn a_reload_whose_file_became_broken_keeps_the_old_revision_with_the_error() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "pic.png", 2, 2, [0, 0, 255, 255]);
    let service = backend(&scratch);
    let key = service.load("pic.png").expect("admitted");
    let mut fake = Backend::default();
    step(&service).expect("a job");
    fake.take_all(&service);
    let before = status(&service, key);
    assert_eq!(before.state, StatusState::Ready);
    fs::write(scratch.path().join("pic.png"), b"broken").expect("a file");
    assert_eq!(service.reload(key), Ok(2));
    step(&service).expect("a job");
    let now = status(&service, key);
    assert_eq!(
        now.state,
        StatusState::Ready,
        "the old revision is still shown"
    );
    assert_eq!(now.shown, before.shown);
    assert_eq!(now.committed, before.committed);
    assert_eq!(now.requested, 2);
    let error = now.error.expect("the error is beside it");
    assert_eq!(error.revision, 2);
    assert!(error.message().contains("not a PNG"), "{}", error.message());
    // A later good file clears it.
    write_png(scratch.path(), "pic.png", 2, 2, [255, 255, 255, 255]);
    assert_eq!(service.reload(key), Ok(3));
    step(&service).expect("a job");
    fake.take_all(&service);
    assert_eq!(status(&service, key).error, None);
    fake.release_all();
    teardown(&service);
}

/// A CPU resource cannot be reloaded, and a file resource cannot have its pixels replaced.
#[test]
fn reload_is_for_file_resources_and_replace_pixels_for_cpu_ones() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "pic.png", 2, 2, [1, 1, 1, 255]);
    let service = headless(&scratch);
    let cpu_pixels = {
        let reservation = service.reserve_pixels(2, 2).expect("reserved");
        let bytes = vec![255; reservation.byte_len()];
        CpuPixels::straight_alpha(reservation, bytes).expect("the right length")
    };
    let cpu = service.create_from_pixels(cpu_pixels).expect("admitted");
    assert_eq!(service.reload(cpu), Err(AdmitError::NotAFileSource));
    let file = service.load("pic.png").expect("admitted");
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bytes = vec![255; reservation.byte_len()];
    let pixels = CpuPixels::straight_alpha(reservation, bytes).expect("the right length");
    let refused = service
        .replace_pixels(file, pixels)
        .expect_err("a file resource is reloaded, not replaced");
    assert_eq!(refused.error, AdmitError::NotACpuSource);
    drop(refused.pixels);
    assert_eq!(
        service.stats().jobs.reserved,
        2,
        "nothing changed: one job slot per resource"
    );
    while step(&service).is_some() {}
    teardown(&service);
}

/// Coalescing as for CPU sources: with a job held, two reloads leave one queued request (the second replaced the first in
/// place and kept its job slot), and only the last result is applied.
#[test]
fn reloads_coalesce_and_only_the_last_result_is_applied() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "pic.png", 2, 2, [5, 5, 5, 255]);
    let service = headless(&scratch);
    let key = service.load("pic.png").expect("admitted");
    let held = service.take_job().expect("revision 1 is executing");
    assert_eq!(service.reload(key), Ok(2));
    assert_eq!(
        service.stats().jobs.reserved,
        2,
        "the executing job and the queued request"
    );
    assert_eq!(service.reload(key), Ok(3));
    assert_eq!(
        service.stats().jobs.reserved,
        2,
        "the second reload replaced the first in place: no third slot"
    );
    service.complete(held.run());
    assert_eq!(
        state(&service, key),
        StatusState::Pending,
        "revision 1's result was superseded and discarded"
    );
    assert_eq!(step(&service), Some((key, 3)), "revision 2 never ran");
    assert_eq!(step(&service), None);
    let now = status(&service, key);
    assert_eq!(now.shown.map(|shown| shown.revision), Some(3));
    teardown(&service);
}

/// Dropping a file resource's last reference cancels its queued request: the job never runs and the slot returns.
#[test]
fn a_file_resource_released_before_its_job_runs_is_retired_with_nothing_run() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "pic.png", 2, 2, [5, 5, 5, 255]);
    let service = headless(&scratch);
    let key = service.load("pic.png").expect("admitted");
    service.release(key).expect("released");
    assert!(service.retirement_pending());
    assert_eq!(service.maintain(8).finished, 1);
    assert!(service.take_job().is_none(), "nothing left to run");
    assert_eq!(reserved(&service), [0, 0, 0]);
    teardown(&service);
}

/// File and CPU requests wait in separate queues (a file may have to wait for CPU bytes, a CPU request may not), and the
/// older head goes first: jobs come out in admission order across the two kinds. Negative control: a service that
/// always preferred one queue would take the CPU request before the first file.
#[test]
fn file_and_cpu_requests_are_taken_in_admission_order_across_the_two_queues() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "pic.png", 2, 2, [5, 5, 5, 255]);
    let service = headless(&scratch);
    let first = service.load("pic.png").expect("admitted");
    let reservation = service.reserve_pixels(2, 2).expect("reserved");
    let bytes = vec![255; reservation.byte_len()];
    let pixels = CpuPixels::straight_alpha(reservation, bytes).expect("the right length");
    let second = service.create_from_pixels(pixels).expect("admitted");
    let third = service.load("pic.png").expect("admitted");
    let mut taken = Vec::new();
    while let Some((key, _)) = step(&service) {
        taken.push(key);
    }
    assert_eq!(taken, vec![first, second, third]);
    teardown(&service);
}

/// CK-129 review 2: the diagnostics counters stop at the maximum and do not wrap to zero.
#[test]
fn the_diagnostics_counters_saturate_instead_of_wrapping() {
    use std::sync::atomic::AtomicU64;

    let counter = AtomicU64::new(u64::MAX - 1);
    bump(&counter, 1);
    assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
    bump(&counter, 5);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        u64::MAX,
        "no wrap to a small number"
    );
}

/// CK-129 (manager-authorized, read-only diagnostics): the loader counters count the files whose decode finished and their decoded
/// bytes, and the deepest the request queues were. A file that is missing decodes nothing and counts nothing. Negative control: a
/// counter bumped at admission instead of at the decode would count the missing file and fail the `1` below.
#[test]
fn the_loader_counters_count_decoded_files_and_bytes_and_the_deepest_queue() {
    let scratch = Scratch::new();
    write_png(scratch.path(), "wide.png", 4, 3, [1, 2, 3, 255]);
    write_png(scratch.path(), "small.png", 2, 2, [7, 7, 7, 255]);
    let service = file_service(scratch.path(), Mode::Headless, limits(1, 64 << 20));
    assert_eq!(
        (
            service.stats().loaders.files_decoded,
            service.stats().loaders.decoded_bytes,
            service.stats().loaders.queue_depth_peak
        ),
        (0, 0, 0)
    );
    let keys =
        ["wide.png", "small.png", "absent.png"].map(|name| service.load(name).expect("admitted"));
    assert_eq!(
        service.stats().loaders.queue_depth_peak,
        3,
        "three requests waited at once"
    );
    for _ in keys {
        let job = service.take_job().expect("a job");
        service.complete(job.run());
    }
    let loaders = service.stats().loaders;
    assert_eq!(loaders.files_decoded, 2, "the missing file decoded nothing");
    assert_eq!(
        loaders.decoded_bytes,
        (4 * 3 + 2 * 2) * 4,
        "RGBA bytes of the two decoded images"
    );
    assert_eq!(loaders.queue_depth_peak, 3, "a peak does not fall");
    teardown(&service);
}

//! Off-screen private texture upload probe. The supervisor starts the worker with Metal validation
//! enabled before the first Metal object is created and gives every process wait a finite deadline.

use std::{
    env,
    ffi::{c_char, c_void},
    process::{Command, ExitCode, Stdio},
    ptr::NonNull,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicI32, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use ck_gpu_metal::{
    BindlessLimits, Completed, LedgerCleanup, LedgerEvent, LedgerLimits, MetalContext,
    OfferRefusal, PixelSource, TableView, TextureLedger, TextureLimits, TextureStore, TextureWakes,
    UploadItem,
};

const WORKER: &str = "--texture-worker";
const DEADLINE: Duration = Duration::from_secs(45);
const VERIFY_DEADLINE: Duration = Duration::from_secs(30);

fn report_class_t_margin(label: &str, elapsed: Duration, threshold: Duration) {
    if env::var("CK139_REPORT_T_MARGINS").as_deref() == Ok("1") {
        eprintln!(
            "CK139 class-T margin {label}: elapsed={elapsed:?}, threshold={threshold:?}, remaining={:?}",
            threshold.saturating_sub(elapsed)
        );
    }
}

unsafe extern "C" {
    fn ck_test_bindless_probe_new(
        context: *mut c_void,
        table: *mut c_void,
        library: *const u8,
        library_length: usize,
        slot: u32,
        hold_completion: i32,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_test_bindless_probe_prepare(
        probe: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_test_bindless_probe_commit(
        probe: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_test_bindless_probe_gpu_done(probe: *mut c_void) -> i32;
    fn ck_test_bindless_probe_read(
        probe: *mut c_void,
        output: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_test_bindless_probe_open_gate(probe: *mut c_void);
    fn ck_test_bindless_table_entry(
        table: *mut c_void,
        slots: u32,
        slot: u32,
        resource_id: *mut u64,
    ) -> i32;
    fn ck_test_bindless_probe_free(probe: *mut c_void);
}

struct BindlessProbe(NonNull<c_void>);

impl BindlessProbe {
    fn new(
        context: &MetalContext,
        table: TableView,
        library: &[u8],
        hold_completion: bool,
    ) -> Result<Self, String> {
        if table.buffer.is_null() || table.slots < 8 {
            return Err("bindless probe requires the first eight table slots".to_owned());
        }
        let mut error = [0_u8; 1024];
        // SAFETY: the active ticket pins this table through the native probe and the library slice stays live for creation.
        let raw = unsafe {
            ck_test_bindless_probe_new(
                context.test_raw_handle(),
                table.buffer,
                library.as_ptr(),
                library.len(),
                0,
                i32::from(hold_completion),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        NonNull::new(raw).map(Self).ok_or_else(|| {
            String::from_utf8_lossy(&error)
                .trim_end_matches('\0')
                .to_owned()
        })
    }

    fn submit(&mut self) -> Result<(), String> {
        let mut error = [0_u8; 1024];
        // SAFETY: this wrapper uniquely owns the unsubmitted probe command.
        let prepared = unsafe {
            ck_test_bindless_probe_prepare(self.0.as_ptr(), error.as_mut_ptr().cast(), error.len())
        };
        if prepared != 0 {
            return Err(String::from_utf8_lossy(&error)
                .trim_end_matches('\0')
                .to_owned());
        }
        // SAFETY: the native probe was prepared and remains uniquely owned here.
        let committed = unsafe {
            ck_test_bindless_probe_commit(self.0.as_ptr(), error.as_mut_ptr().cast(), error.len())
        };
        if committed == 0 {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&error)
                .trim_end_matches('\0')
                .to_owned())
        }
    }

    fn gpu_done(&self) -> bool {
        // SAFETY: the probe holder remains live for this synchronous status read.
        unsafe { ck_test_bindless_probe_gpu_done(self.0.as_ptr()) != 0 }
    }

    fn open_gate(&self) {
        // SAFETY: the probe holder owns the completion gate.
        unsafe { ck_test_bindless_probe_open_gate(self.0.as_ptr()) };
    }

    fn readback(&self) -> Result<Vec<u8>, String> {
        let mut output = vec![0_u8; 8 * 4];
        let mut error = [0_u8; 1024];
        // SAFETY: output has the probe's exact eight-pixel length and the native holder is live.
        let result = unsafe {
            ck_test_bindless_probe_read(
                self.0.as_ptr(),
                output.as_mut_ptr(),
                output.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(output)
        } else {
            Err(String::from_utf8_lossy(&error)
                .trim_end_matches('\0')
                .to_owned())
        }
    }
}

impl Drop for BindlessProbe {
    fn drop(&mut self) {
        // SAFETY: consumes this unique test-only native probe holder and opens its gate first.
        unsafe { ck_test_bindless_probe_free(self.0.as_ptr()) };
    }
}

fn table_resource_id(table: TableView, slot: u32) -> Result<u64, String> {
    if table.buffer.is_null() || slot >= table.slots {
        return Err("invalid bindless table entry".to_owned());
    }
    let mut resource_id = 0_u64;
    // SAFETY: the table's frame ticket remains live and the native helper checks its bounds.
    let result = unsafe {
        ck_test_bindless_table_entry(table.buffer, table.slots, slot, &raw mut resource_id)
    };
    if result == 0 {
        Ok(resource_id)
    } else {
        Err("native bindless table entry read failed".to_owned())
    }
}

#[derive(Clone)]
struct Pixels {
    bytes: Vec<u8>,
    drops: Option<Arc<AtomicUsize>>,
}
struct PanickingPayload;
impl Drop for PanickingPayload {
    fn drop(&mut self) {
        panic!("negative control: panic payload drop");
    }
}
impl PixelSource for Pixels {
    fn pixels(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for Pixels {
    fn drop(&mut self) {
        if let Some(drops) = &self.drops {
            drops.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the supervised worker keeps ordered footprint, pixel and throughput measurements together"
)]
fn worker() -> ExitCode {
    if env::var("MTL_DEBUG_LAYER").as_deref() != Ok("1")
        || env::var("MTL_SHADER_VALIDATION").as_deref() != Ok("1")
    {
        eprintln!("texture worker: Metal validation variables missing");
        return ExitCode::from(2);
    }
    let context = match MetalContext::new() {
        Ok(context) => context,
        Err(error) if error.to_string() == "No Metal device is available" => {
            eprintln!("texture worker: {error}");
            return ExitCode::from(77);
        }
        Err(error) => {
            eprintln!("texture worker: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !verify_hooks_do_not_hold_other_completions(&context) {
        return ExitCode::FAILURE;
    }
    let estimate = context.heap_texture_size_estimate(32, 32);
    let (port, mut owner) = match TextureStore::start(
        Arc::clone(&context),
        TextureLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("texture worker: {error}");
            return ExitCode::FAILURE;
        }
    };
    let bytes: Vec<u8> = (0_u16..32 * 32 * 4)
        .map(|i| (i % 251).to_le_bytes()[0])
        .collect();
    let pixels = Arc::new(Pixels {
        bytes: bytes.clone(),
        drops: None,
    });
    if port
        .offer(UploadItem {
            tag: 1_u32,
            width: 32,
            height: 32,
            pixels,
        })
        .is_err()
    {
        eprintln!("texture worker: first offer refused");
        return ExitCode::FAILURE;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    loop {
        owner.submit();
        owner.poll(&mut completed);
        if !completed.is_empty() {
            break;
        }
        if Instant::now() >= until {
            eprintln!("texture worker: upload timed out");
            return ExitCode::FAILURE;
        }
        thread::yield_now();
    }
    let item = completed.pop().expect("one result");
    let Completed::Ready(mut texture) = item else {
        eprintln!("texture worker: upload failed");
        return ExitCode::FAILURE;
    };
    println!(
        "measurement: 32x32 private texture allocatedSize={} heapEstimate={estimate}",
        texture.allocated_size
    );
    match texture.readback() {
        Ok(actual) if actual == bytes => println!("pixels: 32x32 byte-for-byte match"),
        Ok(_) => {
            eprintln!("texture worker: readback mismatch");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("texture worker: readback failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    drop(texture);
    if !verify_pixel_matrix(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_failure_paths(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_revision_ledger(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_bindless_residency_ledger(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_bindless_table_probe(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_charge_cap(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_failures(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_compat_hold(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_compat_slot_exhaustion(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_separate_completion_and_retirement(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_review_regressions(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_ledger_resolves_many_keys_linearly(&context) {
        return ExitCode::FAILURE;
    }
    if !verify_spare_ticket_maps(&context)
        || !verify_spare_oversized_map_is_not_kept(&context)
        || !verify_spare_limit_is_fixed_at_setup(&context)
    {
        return ExitCode::FAILURE;
    }
    let (private_rate, shared_images) = match measure_throughput(&port, &mut owner) {
        Ok(measurement) => measurement,
        Err(error) => {
            eprintln!("texture worker: throughput measurement failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let shared_elapsed = match context.measure_shared_replacement(32, 32, 2048, &shared_images) {
        Ok(elapsed) => elapsed,
        Err(error) => {
            eprintln!("texture worker: shared alternative failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let shared_rate = 2048.0 / shared_elapsed.as_secs_f64();
    println!(
        "measurement: private+blit 2048 images/s={private_rate:.1} (provisional target 1024/s)"
    );
    println!("measurement: shared+replaceRegion 2048 images/s={shared_rate:.1}");
    match owner.shutdown(Instant::now() + Duration::from_secs(3)) {
        ck_gpu_metal::StoreCleanup::Proven => ExitCode::SUCCESS,
        ck_gpu_metal::StoreCleanup::Unproven { reason } => {
            eprintln!("texture worker: shutdown unproven: {reason}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookInterferenceArm {
    CompletionGate,
    UploadDelay,
    ReaderDelay,
}

fn offer_hook_probe(ledger: &TextureLedger<u32>, key: u32) -> Result<(), String> {
    ledger
        .offer(
            key,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![key.to_le_bytes()[0], 2, 3, 255],
                drops: None,
            }),
        )
        .map_err(|error| format!("offer key {key}: {error:?}"))
}

fn submit_hook_probe(
    ledger: &TextureLedger<u32>,
    key: u32,
    events: &mut Vec<LedgerEvent<u32>>,
) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(3);
    while ledger.upload_stats().batches_submitted == 0 && Instant::now() < until {
        ledger.pump(events);
        events.clear();
        thread::yield_now();
    }
    if ledger.upload_stats().batches_submitted != 1 {
        return Err(format!("key {key} upload was not submitted"));
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the three hook arms share one ordered A/B control and teardown path"
)]
fn verify_hooks_do_not_hold_other_completions(context: &Arc<MetalContext>) -> bool {
    for arm in [
        HookInterferenceArm::CompletionGate,
        HookInterferenceArm::UploadDelay,
        HookInterferenceArm::ReaderDelay,
    ] {
        let gate = Arc::new(AtomicI32::new(0));
        let hooks = match arm {
            HookInterferenceArm::CompletionGate => ck_gpu_metal::TextureTestHooks {
                completion_gate: Some(Arc::clone(&gate)),
                ..Default::default()
            },
            HookInterferenceArm::UploadDelay => ck_gpu_metal::TextureTestHooks {
                completion_delay: Duration::from_secs(5),
                ..Default::default()
            },
            HookInterferenceArm::ReaderDelay => ck_gpu_metal::TextureTestHooks::default(),
        };
        let a = match TextureLedger::start_with_test_hooks(
            Arc::clone(context),
            TextureLimits::default(),
            LedgerLimits::default(),
            TextureWakes::default(),
            hooks,
        ) {
            Ok(ledger) => ledger,
            Err(error) => {
                eprintln!("hook interference {arm:?}: ledger A start failed: {error}");
                return false;
            }
        };
        let b = match TextureLedger::start(
            Arc::clone(context),
            TextureLimits::default(),
            LedgerLimits::default(),
            TextureWakes::default(),
        ) {
            Ok(ledger) => ledger,
            Err(error) => {
                eprintln!("hook interference {arm:?}: ledger B start failed: {error}");
                return false;
            }
        };
        let mut a_events = Vec::new();
        let mut b_events = Vec::new();
        let mut reader = None;
        if arm == HookInterferenceArm::ReaderDelay {
            if offer_hook_probe(&a, 10).is_err()
                || !wait_ledger_prepared(context, &a, 10, 1, &mut a_events)
                || a.commit(10, 1).is_err()
            {
                eprintln!("hook interference {arm:?}: reader source did not prepare");
                return false;
            }
            let mut adopted = Vec::new();
            a.adopt(1, &mut adopted);
            let frame = match a.begin_frame() {
                Ok(frame) => frame,
                Err(error) => {
                    eprintln!("hook interference {arm:?}: frame ticket failed: {error:?}");
                    return false;
                }
            };
            let Some(texture) = frame.resolve(10) else {
                eprintln!("hook interference {arm:?}: source texture did not resolve");
                return false;
            };
            let mut command = match texture.test_reader(Duration::from_secs(5), false) {
                Ok(command) => command,
                Err(error) => {
                    eprintln!("hook interference {arm:?}: reader creation failed: {error}");
                    return false;
                }
            };
            drop(texture);
            if frame.prepare_submit(&mut command).is_err()
                || command.commit().is_err()
                || frame.submitted().is_err()
            {
                eprintln!("hook interference {arm:?}: reader submission failed");
                return false;
            }
            reader = Some(command);
            a_events.clear();
        } else if offer_hook_probe(&a, 10).is_err()
            || submit_hook_probe(&a, 10, &mut a_events).is_err()
        {
            eprintln!("hook interference {arm:?}: hooked upload A did not submit");
            return false;
        }
        if offer_hook_probe(&b, 20).is_err() || submit_hook_probe(&b, 20, &mut b_events).is_err() {
            eprintln!("hook interference {arm:?}: plain upload B did not submit");
            return false;
        }
        let b_prepared = wait_ledger_prepared(context, &b, 20, 1, &mut b_events);
        let a_published = match arm {
            HookInterferenceArm::CompletionGate | HookInterferenceArm::UploadDelay => {
                a.completion_arrived_unpolled()
            }
            HookInterferenceArm::ReaderDelay => {
                reader.as_ref().is_some_and(|item| item.readback().is_ok())
            }
        };
        let isolated = b_prepared && !a_published;
        eprintln!(
            "hook isolation after-fix {arm:?}: B_prepared={b_prepared}, A_published={a_published}, isolated={isolated}"
        );
        if arm == HookInterferenceArm::CompletionGate {
            gate.store(1, Ordering::Release);
        }
        let cleanup_deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < cleanup_deadline {
            a.pump(&mut a_events);
            b.pump(&mut b_events);
            let a_finished = match arm {
                HookInterferenceArm::CompletionGate | HookInterferenceArm::UploadDelay => {
                    a_events.iter().any(|event| {
                        matches!(
                            event,
                            LedgerEvent::Prepared {
                                key: 10,
                                revision: 1
                            }
                        )
                    })
                }
                HookInterferenceArm::ReaderDelay => {
                    reader.as_ref().is_some_and(|item| item.readback().is_ok())
                }
            };
            if a_finished
                && b_events.iter().any(|event| {
                    matches!(
                        event,
                        LedgerEvent::Prepared {
                            key: 20,
                            revision: 1
                        }
                    )
                })
            {
                break;
            }
            thread::yield_now();
        }
        drop(reader);
        let a_cleanup = a.shutdown(Instant::now() + Duration::from_secs(5));
        let b_cleanup = b.shutdown(Instant::now() + Duration::from_secs(5));
        if a_cleanup != LedgerCleanup::Proven || b_cleanup != LedgerCleanup::Proven {
            eprintln!("hook interference {arm:?}: cleanup A={a_cleanup:?}, B={b_cleanup:?}");
            return false;
        }
        if !isolated {
            eprintln!(
                "hook interference {arm:?}: plain completion was not published while hooked completion remained pending"
            );
            return false;
        }
    }
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the transition-table regression run asserts ordered charge and ownership changes"
)]
fn verify_revision_ledger(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;

    let limits = LedgerLimits {
        max_extra_revisions: 32,
        max_adoptions_per_frame: 2,
    };
    if (LedgerLimits {
        max_extra_revisions: 0,
        ..limits
    })
    .validate()
    .is_ok()
        || (LedgerLimits {
            max_adoptions_per_frame: 65_537,
            ..limits
        })
        .validate()
        .is_ok()
    {
        eprintln!("ledger: invalid limits were accepted");
        return false;
    }
    let ledger = match TextureLedger::<u32>::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        limits,
        TextureWakes::default(),
        TextureTestHooks::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger: start failed: {error}");
            return false;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    let offer = |ledger: &TextureLedger<u32>, key, revision, color| {
        ledger.offer(
            key,
            revision,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![color, color.wrapping_add(1), color.wrapping_add(2), 255],
                drops: None,
            }),
        )
    };
    let wait_prepared = |ledger: &TextureLedger<u32>,
                         key,
                         revision,
                         events: &mut Vec<LedgerEvent<u32>>| {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            ledger.pump(events);
            if events.iter().any(|event| matches!(event, LedgerEvent::Prepared { key: found, revision: found_revision } if *found == key && *found_revision == revision)) {
                events.clear();
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            thread::yield_now();
        }
    };
    if offer(&ledger, 1, 1, 10).is_err()
        || !wait_prepared(&ledger, 1, 1, &mut events)
        || ledger.commit(1, 1).is_err()
    {
        eprintln!("ledger: L1/L3/L5 initial revision failed");
        return false;
    }
    let mut adopted = Vec::new();
    ledger.adopt(2, &mut adopted);
    if adopted != [(1, 1)]
        || ledger.stats().baseline_charges != 1
        || ledger.stats().extra_revisions != 0
    {
        eprintln!("ledger: L7 initial adoption or baseline charge mismatch");
        return false;
    }

    // L8/L9/L12: an old submitted frame reads revision 1 while revision 2 is adopted.
    if offer(&ledger, 1, 2, 30).is_err()
        || !wait_prepared(&ledger, 1, 2, &mut events)
        || ledger.commit(1, 2).is_err()
    {
        eprintln!("ledger: replacement preparation/commit failed");
        return false;
    }
    let (mut command, ticket) =
        match begin_ledger_reader(&ledger, 1, Duration::from_millis(600), false) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("ledger: frame setup: {error}");
                return false;
            }
        };
    if ledger.begin_frame().is_ok() || ledger.stats().live_pins != 1 {
        eprintln!("ledger: L17 second building ticket was not refused without changing pins");
        return false;
    }
    if ticket.prepare_submit(&mut command).is_err()
        || command.commit().is_err()
        || ticket.submitted().is_err()
    {
        eprintln!("ledger: L9 reader submission failed");
        return false;
    }
    let pending_ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger: pending-ticket overlap was refused too early: {error:?}");
            return false;
        }
    };
    let Some(pending_texture) = pending_ticket.resolve(1) else {
        eprintln!("ledger: pending-ticket overlap could not resolve active texture");
        return false;
    };
    let mut pending_command = match pending_texture.test_reader(Duration::ZERO, false) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("ledger: pending-ticket reader creation failed: {error}");
            return false;
        }
    };
    drop(pending_texture);
    if !matches!(
        pending_ticket.prepare_submit(&mut pending_command),
        Err(ck_gpu_metal::LedgerError::TicketBusy)
    ) || pending_ticket.discarded().is_err()
    {
        eprintln!("ledger: prepare_submit did not refuse before an earlier token completed");
        return false;
    }
    adopted.clear();
    ledger.adopt(2, &mut adopted);
    if adopted != [(1, 2)] || ledger.stats().extra_revisions != 1 || ledger.stats().retired != 1 {
        eprintln!(
            "ledger: L7 charge swap or pinned retirement failed: adopted={adopted:?}, stats={:?}",
            ledger.stats()
        );
        return false;
    }
    let started = Instant::now();
    ledger.pump(&mut events);
    let pump_elapsed = started.elapsed();
    report_class_t_margin(
        "delayed frame pump",
        pump_elapsed,
        Duration::from_millis(100),
    );
    if pump_elapsed > Duration::from_millis(100) || command.readback().is_ok() {
        eprintln!("ledger: pump waited or readback succeeded before completion");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(4);
    while command.readback().is_err() && Instant::now() < until {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    if command.readback().ok().as_deref() != Some(&[10, 11, 12, 255][..]) {
        eprintln!("ledger: delayed old frame did not read revision 1 pixels");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(2);
    while ledger.stats().tickets_submitted_pending != 0 && Instant::now() < until {
        ledger.pump(&mut events);
    }
    ledger.pump(&mut events);
    if ledger.stats().extra_revisions != 0 || ledger.stats().releases == 0 {
        eprintln!("ledger: L12 failed to release old revision after token completion");
        return false;
    }
    let mut active = match begin_ledger_reader(&ledger, 1, Duration::ZERO, false) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("ledger: new frame setup: {error}");
            return false;
        }
    };
    if active.1.prepare_submit(&mut active.0).is_err()
        || active.0.commit().is_err()
        || active.1.submitted().is_err()
    {
        eprintln!("ledger: second frame failed");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(2);
    while active.0.readback().is_err() && Instant::now() < until {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    if active.0.readback().ok().as_deref() != Some(&[30, 31, 32, 255][..]) {
        eprintln!("ledger: frame after adoption did not read revision 2");
        return false;
    }
    while ledger.stats().tickets_submitted_pending != 0 && Instant::now() < until {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    let ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger: not-committed ticket setup: {error:?}");
            return false;
        }
    };
    let Some(texture) = ticket.resolve(1) else {
        eprintln!("ledger: not-committed resolve failed");
        return false;
    };
    let mut uncommitted = match texture.test_reader(Duration::ZERO, false) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("ledger: not-committed reader creation failed: {error}");
            return false;
        }
    };
    drop(texture);
    if ticket.prepare_submit(&mut uncommitted).is_err() || ticket.not_committed().is_err() {
        eprintln!("ledger: L10 not_committed did not end its pins");
        return false;
    }
    drop(uncommitted);
    if ledger.stats().live_pins != 0 {
        eprintln!("ledger: not_committed leaked frame pins");
        return false;
    }

    // L6 reject-before-commit and L5 replacement of an older committed candidate.
    for (revision, color) in [(3, 50), (4, 70)] {
        if offer(&ledger, 1, revision, color).is_err()
            || !wait_prepared(&ledger, 1, revision, &mut events)
        {
            eprintln!("ledger: candidate {revision} was not prepared");
            return false;
        }
        if revision == 3 {
            if ledger.reject(1, revision).is_err() {
                eprintln!("ledger: L6 reject failed");
                return false;
            }
            ledger.pump(&mut events);
            if ledger.stats().prepared != 0 || ledger.stats().active != 1 {
                eprintln!("ledger: rejected revision became visible");
                return false;
            }
        } else if ledger.commit(1, revision).is_err() {
            eprintln!("ledger: commit after reject failed");
            return false;
        }
    }
    for (revision, color) in [(5, 80), (6, 85)] {
        if offer(&ledger, 1, revision, color).is_err()
            || !wait_prepared(&ledger, 1, revision, &mut events)
            || ledger.commit(1, revision).is_err()
        {
            eprintln!("ledger: second pre-boundary commit {revision} failed");
            return false;
        }
    }
    ledger.pump(&mut events);
    adopted.clear();
    ledger.adopt(2, &mut adopted);
    if adopted != [(1, 6)] || ledger.stats().committed_waiting_for_boundary != 0 {
        eprintln!("ledger: L5 older committed candidate survived the newer commit: {adopted:?}");
        return false;
    }
    for (key, color) in [(10, 90), (11, 100), (12, 110), (13, 120), (14, 130)] {
        if offer(&ledger, key, 1, color).is_err()
            || !wait_prepared(&ledger, key, 1, &mut events)
            || ledger.commit(key, 1).is_err()
        {
            eprintln!("ledger: adoption-limit setup failed for {key}");
            return false;
        }
    }
    adopted.clear();
    ledger.adopt(usize::MAX, &mut adopted);
    if adopted.len() != 2 || ledger.stats().adoptions_last_boundary != 2 {
        eprintln!("ledger: adoption cap was not enforced");
        return false;
    }
    adopted.clear();
    ledger.adopt(usize::MAX, &mut adopted);
    if adopted.len() != 2 {
        eprintln!("ledger: second adoption boundary wrong");
        return false;
    }
    adopted.clear();
    ledger.adopt(usize::MAX, &mut adopted);
    if adopted.len() != 1 {
        eprintln!("ledger: third adoption boundary wrong");
        return false;
    }

    // L10 discarded pin, L13/L14 retire without later caller action; in-flight completion joins retirement.
    let ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger: discard ticket: {error:?}");
            return false;
        }
    };
    let Some(texture) = ticket.resolve(10) else {
        eprintln!("ledger: key 10 inactive");
        return false;
    };
    drop(texture);
    ticket.discarded().ok();
    ledger.retire_key(10);
    if !matches!(
        ledger.offer(
            10,
            2,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![1, 2, 3, 255],
                drops: None
            })
        ),
        Err(OfferRefusal::KeyRetiring)
    ) {
        eprintln!("ledger: L2 retiring-key refusal failed");
        return false;
    }
    loop {
        ledger.pump(&mut events);
        if events
            .iter()
            .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 10 }))
        {
            break;
        }
        if Instant::now() >= deadline {
            eprintln!("ledger: L14 retirement event did not arrive");
            return false;
        }
        thread::yield_now();
    }
    events.clear();

    // Prove with an outstanding submitted frame is non-destructive; completion then permits release.
    let (mut reader, ticket) =
        match begin_ledger_reader(&ledger, 1, Duration::from_millis(100), false) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("ledger: proof reader setup: {error}");
                return false;
            }
        };
    if ticket.prepare_submit(&mut reader).is_err()
        || reader.commit().is_err()
        || ticket.submitted().is_err()
    {
        eprintln!("ledger: proof reader submit failed");
        return false;
    }
    ledger.retire_key(1);
    let native_before = context.native_live_objects();
    let proof = ledger.prove(Instant::now() + Duration::from_millis(10));
    if proof.is_complete() || context.native_live_objects() != native_before {
        eprintln!("ledger: L16 incomplete proof released native objects");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(2);
    while reader.readback().is_err() && Instant::now() < until {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    ledger.retire_key(11);
    ledger.retire_key(12);
    ledger.retire_key(13);
    ledger.retire_key(14);
    let mut end_events = Vec::new();
    let until = Instant::now() + Duration::from_secs(2);
    while ledger.stats().active
        + ledger.stats().retired
        + ledger.stats().committed_waiting_for_boundary
        > 0
        && Instant::now() < until
    {
        ledger.pump(&mut end_events);
    }
    match ledger.shutdown(Instant::now() + Duration::from_secs(3)) {
        LedgerCleanup::Proven => {}
        LedgerCleanup::Unproven { reason } => {
            eprintln!("ledger: final proof failed: {reason}");
            return false;
        }
    }
    if ledger.stats().upload.texture_bytes_reserved != 0 || ledger.stats().upload.staging_bytes != 0
    {
        eprintln!("ledger: proven shutdown left upload charges");
        return false;
    }

    // L11: uncertain tickets retain their pins and native graph for process lifetime.
    let uncertain = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        limits,
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger: uncertain setup: {error}");
            return false;
        }
    };
    if offer(&uncertain, 77, 1, 140).is_err()
        || !wait_prepared(&uncertain, 77, 1, &mut events)
        || uncertain.commit(77, 1).is_err()
    {
        eprintln!("ledger: uncertain setup upload failed");
        return false;
    }
    adopted.clear();
    uncertain.adopt(2, &mut adopted);
    let ticket = match uncertain.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger: uncertain ticket: {error:?}");
            return false;
        }
    };
    let Some(_texture) = ticket.resolve(77) else {
        eprintln!("ledger: uncertain resolve failed");
        return false;
    };
    let proof = uncertain.prove(Instant::now() + Duration::from_millis(5));
    if proof.is_complete() {
        eprintln!("ledger: live building ticket was proven complete");
        return false;
    }
    drop(ticket);
    let mut failure_events = Vec::new();
    uncertain.pump(&mut failure_events);
    if !failure_events
        .iter()
        .any(|e| matches!(e, LedgerEvent::GpuFailure { .. }))
        || !matches!(
            uncertain.shutdown(Instant::now() + Duration::from_millis(5)),
            LedgerCleanup::Unproven { .. }
        )
        || uncertain.stats().tickets_uncertain != 1
    {
        eprintln!("ledger: L11 uncertain outcome was not retained and latched");
        return false;
    }
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the two independent GPU failure paths share one cleanup assertion"
)]
fn verify_ledger_failures(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let limits = LedgerLimits::default();
    let upload = match TextureLedger::<u32>::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        limits,
        TextureWakes::default(),
        TextureTestHooks {
            error_completion: true,
            ..TextureTestHooks::default()
        },
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger failure: upload ledger start failed: {error}");
            return false;
        }
    };
    if upload
        .offer(
            1,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![1, 2, 3, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        eprintln!("ledger failure: upload offer failed");
        return false;
    }
    let mut events = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::GpuFailure { .. }))
    {
        upload.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger failure: upload error was not surfaced");
            return false;
        }
        thread::yield_now();
    }
    upload.pump(&mut events);
    if events
        .iter()
        .filter(|event| matches!(event, LedgerEvent::GpuFailure { .. }))
        .count()
        != 1
        || !events.iter().any(|event| {
            matches!(
                event,
                LedgerEvent::UploadFailed {
                    key: 1,
                    revision: 1,
                    ..
                }
            )
        })
        || !matches!(
            upload.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!("ledger failure: L4/L15 upload failure was duplicated or not proven safely");
        return false;
    }

    let frame = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        limits,
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger failure: frame ledger start failed: {error}");
            return false;
        }
    };
    if frame
        .offer(
            2,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![9, 8, 7, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        eprintln!("ledger failure: frame upload offer failed");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        frame.pump(&mut events);
        if events.iter().any(|event| {
            matches!(
                event,
                LedgerEvent::Prepared {
                    key: 2,
                    revision: 1
                }
            )
        }) {
            break;
        }
        if Instant::now() >= until {
            eprintln!("ledger failure: frame upload timed out");
            return false;
        }
        thread::yield_now();
    }
    if frame.commit(2, 1).is_err() {
        eprintln!("ledger failure: frame commit failed");
        return false;
    }
    let mut adopted = Vec::new();
    frame.adopt(1, &mut adopted);
    let (mut command, ticket) = match begin_ledger_reader(&frame, 2, Duration::ZERO, true) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("ledger failure: frame reader: {error}");
            return false;
        }
    };
    if ticket.prepare_submit(&mut command).is_err()
        || command.commit().is_err()
        || ticket.submitted().is_err()
    {
        eprintln!("ledger failure: frame error token submission failed");
        return false;
    }
    events.clear();
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::GpuFailure { .. }))
    {
        frame.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger failure: frame token error was not surfaced");
            return false;
        }
        thread::yield_now();
    }
    frame.pump(&mut events);
    if events
        .iter()
        .filter(|event| matches!(event, LedgerEvent::GpuFailure { .. }))
        .count()
        != 1
        || !matches!(
            frame.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!("ledger failure: L15 frame failure was duplicated or left an unproven graph");
        return false;
    }
    true
}

/// CK-119 on CK-127's bindless ledger: a revision that cannot get a native texture slot fails like any other per-item resource
/// error, so when it is the marked compatibility key the ledger also reports `CompatUploadFailed` (the presenter's one `Failed`),
/// next to the ordinary `UploadFailed`; an unmarked key reports only the ordinary one.
fn verify_ledger_compat_slot_exhaustion(context: &Arc<MetalContext>) -> bool {
    let pixels = |seed: u8| {
        Arc::new(Pixels {
            bytes: vec![seed, 2, 3, 255],
            drops: None,
        })
    };
    for (marked, expected, label) in [(true, 1_usize, "marked"), (false, 0, "unmarked")] {
        let ledger = match TextureLedger::<u32>::start_bindless(
            Arc::clone(context),
            TextureLimits::default(),
            LedgerLimits::default(),
            BindlessLimits {
                texture_slots: 2,
                max_textures: 1,
                max_extra_revisions: 0,
            },
            128 * 1024 * 1024,
            0,
            TextureWakes::default(),
            ck_gpu_metal::TextureTestHooks::default(),
        ) {
            Ok(ledger) => ledger,
            Err(error) => {
                eprintln!("compat slot exhaustion: {label}: start failed: {error}");
                return false;
            }
        };
        let mut events = Vec::new();
        if ledger.offer(1, 1, 1, 1, pixels(1)).is_err()
            || !wait_ledger_prepared(context, &ledger, 1, 1, &mut events)
            || ledger.commit(1, 1).is_err()
        {
            eprintln!("compat slot exhaustion: {label}: the first revision was not prepared");
            return false;
        }
        let mut adopted = Vec::new();
        ledger.adopt(1, &mut adopted);
        if adopted != [(1, 1)] {
            return false;
        }
        // The slots are used up: the second key's upload fails when it would get its slot.
        if marked {
            ledger.set_compat_key(Some(2));
        }
        events.clear();
        if ledger.offer(2, 1, 1, 1, pixels(4)).is_err() {
            return false;
        }
        let until = Instant::now() + Duration::from_secs(5);
        while !events
            .iter()
            .any(|event| matches!(event, LedgerEvent::UploadFailed { key: 2, .. }))
        {
            ledger.pump(&mut events);
            if Instant::now() >= until {
                eprintln!("compat slot exhaustion: {label}: the failure never arrived");
                return false;
            }
            thread::yield_now();
        }
        ledger.pump(&mut events);
        let compat = events
            .iter()
            .filter(|event| matches!(event, LedgerEvent::CompatUploadFailed { key: 2, .. }))
            .count();
        let ordinary = events
            .iter()
            .filter(|event| matches!(event, LedgerEvent::UploadFailed { key: 2, reason, .. } if reason.contains("slots exhausted")))
            .count();
        if compat != expected
            || ordinary != 1
            || !matches!(
                ledger.shutdown(Instant::now() + Duration::from_secs(3)),
                LedgerCleanup::Proven
            )
        {
            eprintln!(
                "compat slot exhaustion: {label}: expected {expected} compat and 1 ordinary failure, saw {compat} and {ordinary}"
            );
            return false;
        }
    }
    true
}

/// CK-119: the compatibility key's marking and failure classification, and the one-slot hold of the last image drawn.
#[expect(
    clippy::too_many_lines,
    reason = "the classification table and the hold's lifetime are one ordered ledger proof"
)]
fn verify_ledger_compat_hold(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let pixel = || {
        Arc::new(Pixels {
            bytes: vec![1, 2, 3, 255],
            drops: None,
        })
    };
    // Classification: a forced per-item failure of key 1 is also `CompatUploadFailed` exactly when key 1 is the marked key
    // at the pump that sees it; the ordinary `UploadFailed` is always there.
    for (marked, cleared, expected, label) in [
        (Some(1_u32), false, 1_usize, "marked"),
        (None, false, 0, "never marked"),
        (Some(2), false, 0, "another key marked"),
        (Some(1), true, 0, "cleared before the pump"),
    ] {
        let ledger = match TextureLedger::<u32>::start_with_test_hooks(
            Arc::clone(context),
            TextureLimits::default(),
            LedgerLimits::default(),
            TextureWakes::default(),
            TextureTestHooks {
                force_estimate_at: Some((1, 1 << 50)),
                ..TextureTestHooks::default()
            },
        ) {
            Ok(ledger) => ledger,
            Err(error) => {
                eprintln!("compat hold: {label}: ledger start failed: {error}");
                return false;
            }
        };
        ledger.set_compat_key(marked);
        if ledger.offer(1, 1, 1, 1, pixel()).is_err() {
            eprintln!("compat hold: {label}: offer failed");
            return false;
        }
        if cleared {
            ledger.set_compat_key(None);
        }
        let mut events = Vec::new();
        let until = Instant::now() + Duration::from_secs(5);
        while !events
            .iter()
            .any(|event| matches!(event, LedgerEvent::UploadFailed { key: 1, .. }))
        {
            ledger.pump(&mut events);
            if Instant::now() >= until {
                eprintln!("compat hold: {label}: the failure never arrived");
                return false;
            }
            thread::yield_now();
        }
        ledger.pump(&mut events);
        let compat = events
            .iter()
            .filter(|event| matches!(event, LedgerEvent::CompatUploadFailed { key: 1, .. }))
            .count();
        let ordinary = events
            .iter()
            .filter(|event| matches!(event, LedgerEvent::UploadFailed { key: 1, .. }))
            .count();
        if compat != expected
            || ordinary != 1
            || !matches!(
                ledger.shutdown(Instant::now() + Duration::from_secs(3)),
                LedgerCleanup::Proven
            )
        {
            eprintln!(
                "compat hold: {label}: expected {expected} compat and 1 ordinary failure, saw {compat} and {ordinary}"
            );
            return false;
        }
    }

    // The hold.
    // Bind a real checker so the revision also has a native table slot; without a bindless
    // table, resolve_revision_slot correctly returns None before the hold changes.
    let mut events = Vec::new();
    let provider = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("compat hold: ledger start failed: {error}");
            return false;
        }
    };
    if provider
        .offer(
            90,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![9, 8, 7, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &provider, 90, 1, &mut events)
        || provider.commit(90, 1).is_err()
    {
        eprintln!("compat hold: checker provider did not prepare");
        return false;
    }
    let mut provider_adopted = Vec::new();
    provider.adopt(1, &mut provider_adopted);
    let checker_ticket = match provider.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("compat hold: checker ticket failed: {error:?}");
            return false;
        }
    };
    let Some(checker) = checker_ticket.resolve(90) else {
        eprintln!("compat hold: checker did not resolve");
        return false;
    };
    let ledger = match TextureLedger::<u32>::start_bindless(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        BindlessLimits {
            texture_slots: 8,
            max_textures: 4,
            max_extra_revisions: 1,
        },
        128 * 1024 * 1024,
        0,
        TextureWakes::default(),
        TextureTestHooks::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("compat hold: bindless ledger start failed: {error}");
            return false;
        }
    };
    if let Err(error) = ledger.test_bind_checker(&checker) {
        eprintln!("compat hold: checker binding failed: {error}");
        return false;
    }
    drop(checker);
    if checker_ticket.discarded().is_err() {
        eprintln!("compat hold: checker ticket did not close");
        return false;
    }
    for key in [1_u32, 2] {
        if ledger.offer(key, 1, 1, 1, pixel()).is_err()
            || !wait_ledger_prepared(context, &ledger, key, 1, &mut events)
        {
            eprintln!("compat hold: key {key} was not prepared");
            return false;
        }
    }
    if ledger.has_committed_or_active(1) {
        eprintln!("compat hold: a prepared revision counts as committed");
        return false;
    }
    if ledger.commit(1, 1).is_err() || ledger.commit(2, 1).is_err() {
        eprintln!("compat hold: commit failed");
        return false;
    }
    if !ledger.has_committed_or_active(1) || ledger.has_committed_or_active(3) {
        eprintln!("compat hold: a committed revision is not seen, or an unknown key is");
        return false;
    }
    let mut adopted = Vec::new();
    ledger.adopt(2, &mut adopted);
    if adopted.len() != 2 || !ledger.has_committed_or_active(1) {
        eprintln!("compat hold: adoption failed: {adopted:?}");
        return false;
    }
    // One slot: holding an unknown revision is refused and changes nothing.
    if ledger.hold_compat(9, 1).is_ok()
        || ledger.hold_compat(1, 7).is_ok()
        || ledger.compat_hold().is_some()
    {
        eprintln!("compat hold: an unknown revision was held");
        return false;
    }
    if ledger.hold_compat(1, 1).is_err()
        || ledger.hold_compat(1, 1).is_err()
        || ledger.compat_hold() != Some((1, 1))
        || ledger.stats().compat_held_revision != 1
    {
        eprintln!(
            "compat hold: the hold was not taken once: {:?}",
            ledger.stats()
        );
        return false;
    }
    // The key retires while its image is held: its retirement waits, and the held revision stays drawable.
    ledger.retire_key(1);
    ledger.pump(&mut events);
    events.clear();
    ledger.pump(&mut events);
    if events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 1 }))
    {
        eprintln!("compat hold: the held key was reported retired");
        return false;
    }
    {
        let Ok(ticket) = ledger.begin_frame() else {
            eprintln!("compat hold: no ticket");
            return false;
        };
        let live = ticket.resolve(1).is_some();
        let held = ticket.resolve_revision(1, 1).is_some();
        let wrong_revision = ticket.resolve_revision(1, 2).is_some();
        let not_held = ticket.resolve_revision(2, 1).is_some();
        if live || !held || wrong_revision || not_held || ticket.discarded().is_err() {
            eprintln!(
                "compat hold: resolution after retirement was live={live} held={held} wrong={wrong_revision} not_held={not_held}"
            );
            return false;
        }
    }
    // Holding the newer image releases the older one (at most one old revision is held), and the retirement is reported.
    if ledger.hold_compat(2, 1).is_err() || ledger.compat_hold() != Some((2, 1)) {
        eprintln!("compat hold: the newer image was not held");
        return false;
    }
    {
        let Ok(ticket) = ledger.begin_frame() else {
            eprintln!("compat hold: no ticket for changed-hold resolution");
            return false;
        };
        let slot_before_release = ticket.resolve_revision_slot(2, 1);
        let texture_before_release = ticket.resolve_revision(2, 1).is_some();
        ledger.release_compat_hold();
        let slot_after_release = ticket.resolve_revision_slot(2, 1);
        let texture_after_release = ticket.resolve_revision(2, 1).is_some();
        if slot_before_release.is_none()
            || !texture_before_release
            || slot_after_release.is_some()
            || texture_after_release
            || ticket.discarded().is_err()
        {
            eprintln!(
                "compat hold: changing the hold did not make repeated held lookups return None: slot {slot_before_release:?}->{slot_after_release:?}, texture {texture_before_release}->{texture_after_release}"
            );
            return false;
        }
    }
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 1 }))
    {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("compat hold: the released key was never reported retired");
            return false;
        }
        thread::yield_now();
    }
    // Releasing the hold lets the key it kept alive retire; the charges are conserved throughout.
    ledger.release_compat_hold();
    ledger.release_compat_hold();
    ledger.retire_key(2);
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 2 }))
    {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("compat hold: the unheld key was never reported retired");
            return false;
        }
        thread::yield_now();
    }
    if ledger.compat_hold().is_some()
        || ledger.stats().compat_held_revision != 0
        || !ledger_charges_conserved(&ledger.stats())
    {
        eprintln!(
            "compat hold: the hold or the charges were left: {:?}",
            ledger.stats()
        );
        return false;
    }
    matches!(
        ledger.shutdown(Instant::now() + Duration::from_secs(3)),
        LedgerCleanup::Proven
    ) && matches!(
        provider.shutdown(Instant::now() + Duration::from_secs(3)),
        LedgerCleanup::Proven
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "this regression orders independent upload and frame proofs around retirement"
)]
fn verify_ledger_separate_completion_and_retirement(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let native_baseline = context.native_live_objects();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let ledger = match TextureLedger::<u32>::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
        TextureTestHooks {
            block_after_texture: Some(Arc::clone(&gate)),
            block_after_texture_at: Some(2),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger separation: start failed: {error}");
            return false;
        }
    };
    let pixels = |value| {
        Arc::new(Pixels {
            bytes: vec![value, value, value, 255],
            drops: None,
        })
    };
    if ledger.offer(1, 1, 1, 1, pixels(11)).is_err() {
        eprintln!("ledger separation: baseline offer failed");
        return false;
    }
    let mut events = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while !events.iter().any(|event| {
        matches!(
            event,
            LedgerEvent::Prepared {
                key: 1,
                revision: 1
            }
        )
    }) {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger separation: baseline upload timed out");
            return false;
        }
        thread::yield_now();
    }
    if ledger.commit(1, 1).is_err() {
        eprintln!("ledger separation: baseline commit failed");
        return false;
    }
    let mut adopted = Vec::new();
    ledger.adopt(1, &mut adopted);
    if adopted != [(1, 1)] || ledger.offer(1, 2, 1, 1, pixels(21)).is_err() {
        eprintln!("ledger separation: replacement offer failed");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(5);
    // The test gate is after native allocation but before the upload stats count it.
    while context.native_live_objects().textures < native_baseline.textures + 2 {
        if Instant::now() >= until {
            eprintln!("ledger separation: replacement never reached its preparation gate");
            return false;
        }
        thread::yield_now();
    }
    let (mut reader, ticket) = match begin_ledger_reader(&ledger, 1, Duration::ZERO, false) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("ledger separation: frame setup failed: {error}");
            return false;
        }
    };
    if ticket.prepare_submit(&mut reader).is_err()
        || reader.commit().is_err()
        || ticket.submitted().is_err()
    {
        eprintln!("ledger separation: frame submission failed");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(5);
    while reader.readback().is_err() {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger separation: frame completion timed out");
            return false;
        }
        thread::yield_now();
    }
    ledger.pump(&mut events);
    if ledger.stats().tickets_submitted_pending != 0
        || ledger.stats().live_pins != 0
        || ledger.stats().in_flight != 1
        || ledger.stats().active != 1
    {
        eprintln!("ledger separation: frame completion ended or hid the still-in-flight upload");
        return false;
    }

    // L13/L14: retire an active pinned revision and a gated in-flight replacement. Pump alone
    // releases both after the frame and upload proofs; no later commit/adopt/offer is needed.
    let (mutex, changed) = &*gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    ledger.retire_key(1);
    if !matches!(
        ledger.offer(1, 3, 1, 1, pixels(31)),
        Err(OfferRefusal::KeyRetiring)
    ) {
        eprintln!("ledger separation: retiring key admitted another offer");
        return false;
    }
    events.clear();
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 1 }))
    {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger separation: pump did not finish key retirement");
            return false;
        }
        thread::yield_now();
    }
    if ledger.stats().extra_revisions != 0
        || ledger.stats().baseline_charges != 0
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!("ledger separation: retirement charges or final proof failed");
        return false;
    }
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the bindless regression checks exhaustion, membership ordering, slot reuse, pass bounds and unproven retention"
)]
fn verify_bindless_residency_ledger(context: &Arc<MetalContext>) -> bool {
    // A spare slot prepares normally; once occupied, exhaustion is transactional and retirement makes
    // the slot reusable only after committed removal and texture release.
    let ledger = match TextureLedger::<u32>::start_bindless(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        BindlessLimits {
            texture_slots: 2,
            max_textures: 1,
            max_extra_revisions: 0,
        },
        128 * 1024 * 1024,
        0,
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("bindless ledger: start failed: {error}");
            return false;
        }
    };
    let baseline = context.native_live_objects();
    let commits_before_prepare = ledger.test_residency_commit_count().unwrap_or_default();
    if ledger
        .offer(
            1,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![1, 2, 3, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        return false;
    }
    let mut events = Vec::new();
    if !wait_ledger_prepared(context, &ledger, 1, 1, &mut events)
        || ledger.stats().residency_commits_last_pass != 1
        || ledger.test_residency_commit_count() != Some(commits_before_prepare + 1)
        || context.native_live_objects().residency_members != baseline.residency_members + 1
    {
        eprintln!("bindless ledger: Prepared was emitted without a committed membership");
        return false;
    }
    events.clear();
    if ledger.commit(1, 1).is_err() {
        return false;
    }
    let mut adopted = Vec::new();
    ledger.adopt(1, &mut adopted);
    if adopted != [(1, 1)] {
        return false;
    }

    let before_exhaustion = context.native_live_objects();
    if ledger
        .offer(
            2,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![4, 5, 6, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        return false;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        ledger.pump(&mut events);
        if events.iter().any(|event| matches!(event, LedgerEvent::UploadFailed { key: 2, revision: 1, reason } if reason == "bindless texture slots exhausted (1 in use, 1 configured)")) { break; }
        if Instant::now() >= deadline {
            eprintln!("bindless ledger: exhaustion event did not arrive");
            return false;
        }
        thread::yield_now();
    }
    if ledger.stats().active != 1
        || ledger.stats().prepared != 0
        || context.native_live_objects().textures != before_exhaustion.textures
        || context.native_live_objects().residency_members != before_exhaustion.residency_members
    {
        eprintln!(
            "bindless ledger: exhaustion changed the shown revision or leaked membership/texture"
        );
        return false;
    }
    let commits_before_retirement = ledger.test_residency_commit_count().unwrap_or_default();
    ledger.retire_key(1);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ledger.stats().retired != 0 && Instant::now() < deadline {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    let removed = context.native_live_objects();
    if ledger.stats().retired != 0
        || removed.last_residency_remove_order <= before_exhaustion.last_residency_remove_order
        || removed.last_texture_release_order <= removed.last_residency_remove_order
        || ledger.stats().residency_commits_last_pass > 1
        || ledger.test_residency_commit_count() != Some(commits_before_retirement + 1)
    {
        eprintln!(
            "bindless ledger: retirement did not commit removal before releasing the texture"
        );
        return false;
    }
    events.clear();
    if ledger
        .offer(
            3,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![7, 8, 9, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 3, 1, &mut events)
    {
        eprintln!("bindless ledger: retired slot was not reusable");
        return false;
    }
    if !matches!(
        ledger.shutdown(Instant::now() + Duration::from_secs(3)),
        LedgerCleanup::Proven
    ) {
        eprintln!("bindless ledger: exhaustion lifecycle cleanup failed");
        return false;
    }

    if let Err(error) = spill_check(context, SpillSchedule::AllQueuedFirst) {
        eprintln!("bindless spill: fixed schedule failed: {error}");
        return false;
    }
    if !matches!(
        spill_check(context, SpillSchedule::EarlySubmit),
        Err(error)
            if error
                == "precondition images_per_batch_max: expected 3, got 1 (batches_prepared=1, batches_submitted=1)"
    ) {
        eprintln!("bindless spill: early-submit control did not fail its named precondition");
        return false;
    }

    // An unproven direct ledger drop must retain its set, resident member and texture, just as
    // the presenter path retains its whole scene. Negative control: normal Drop detaches the set.
    let unproven = match TextureLedger::<u32>::start_bindless(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        BindlessLimits {
            texture_slots: 3,
            max_textures: 2,
            max_extra_revisions: 0,
        },
        128 * 1024 * 1024,
        0,
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("bindless unproven: start failed: {error}");
            return false;
        }
    };
    if unproven
        .offer(
            9,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![9, 8, 7, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        return false;
    }
    let mut events = Vec::new();
    if !wait_ledger_prepared(context, &unproven, 9, 1, &mut events)
        || unproven.commit(9, 1).is_err()
    {
        return false;
    }
    let mut adopted = Vec::new();
    unproven.adopt(1, &mut adopted);
    let (mut reader, ticket) =
        match begin_ledger_reader(&unproven, 9, Duration::from_secs(5), false) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("bindless unproven: reader failed: {error}");
                return false;
            }
        };
    if ticket.prepare_submit(&mut reader).is_err()
        || reader.commit().is_err()
        || ticket.submitted().is_err()
    {
        return false;
    }
    let retained = context.native_live_objects();
    if retained.residency_sets == 0
        || retained.residency_members == 0
        || retained.textures == 0
        || !matches!(
            unproven.shutdown(Instant::now() + Duration::from_millis(10)),
            LedgerCleanup::Unproven { .. }
        )
        || context.native_live_objects() != retained
    {
        eprintln!("bindless unproven: shutdown did not preserve pending resources");
        return false;
    }
    drop(unproven);
    if context.native_live_objects() != retained {
        eprintln!(
            "bindless unproven: dropping the ledger detached or released residency resources"
        );
        return false;
    }
    drop(reader);
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the probe walks one ordered publication, pin, retirement and shutdown lifecycle"
)]
fn verify_bindless_table_probe(context: &Arc<MetalContext>) -> bool {
    const LIBRARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bindless_probe.metallib"));
    let provider = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("bindless probe: checker provider start failed: {error}");
            return false;
        }
    };
    if provider
        .offer(
            90,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![9, 8, 7, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        return false;
    }
    let mut events = Vec::new();
    if !wait_ledger_prepared(context, &provider, 90, 1, &mut events)
        || provider.commit(90, 1).is_err()
    {
        eprintln!("bindless probe: checker texture did not prepare");
        return false;
    }
    let mut adopted = Vec::new();
    provider.adopt(1, &mut adopted);
    let checker_ticket = match provider.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("bindless probe: checker ticket failed: {error:?}");
            return false;
        }
    };
    let Some(checker) = checker_ticket.resolve(90) else {
        eprintln!("bindless probe: checker texture did not resolve");
        return false;
    };
    let ledger = match TextureLedger::<u32>::start_bindless(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        BindlessLimits {
            texture_slots: 8,
            max_textures: 4,
            max_extra_revisions: 2,
        },
        128 * 1024 * 1024,
        0,
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("bindless probe: ledger start failed: {error}");
            return false;
        }
    };
    // Prepare before checker binding, as the presenter can pump texture uploads while its
    // first frame is still waiting for the checker to become resident.
    if ledger
        .offer(
            1,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![31, 41, 51, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 1, 1, &mut events)
    {
        eprintln!("bindless probe: pre-checker revision did not prepare");
        return false;
    }
    let Some(first_slot) = ledger.test_native_slot(1, 1) else {
        eprintln!("bindless probe: pre-checker revision had no native slot");
        return false;
    };
    if first_slot != 1 {
        eprintln!("bindless probe: pre-checker revision used unexpected probe slot {first_slot}");
        return false;
    }
    if let Err(error) = ledger.test_bind_checker(&checker) {
        eprintln!("bindless probe: checker binding failed: {error}");
        return false;
    }
    drop(checker);
    let Some(initial) = ledger.test_bindless_entries(0) else {
        eprintln!("bindless probe: initial table values unavailable");
        return false;
    };
    if initial[0] == 0 || initial[0] != initial[1] {
        eprintln!("bindless probe: both tables were not initialized to the checker");
        return false;
    }
    let checker_pixel = [9, 8, 7, 255];
    if !check_table_probe(context, &ledger, LIBRARY, 0, [checker_pixel; 8]) {
        eprintln!("bindless probe: pre-checker staged entry became visible before commit");
        return false;
    }

    if ledger.commit(1, 1).is_err() {
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let mut first_revision_pixels = [checker_pixel; 8];
    first_revision_pixels[first_slot as usize] = [31, 41, 51, 255];
    if adopted != [(1, 1)]
        || !check_table_probe(context, &ledger, LIBRARY, first_slot, first_revision_pixels)
    {
        eprintln!("bindless probe: first publication did not switch the sampled table epoch");
        return false;
    }

    if ledger
        .offer(
            3,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![15, 25, 35, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 3, 1, &mut events)
        || ledger.commit(3, 1).is_err()
    {
        eprintln!("bindless probe: third active revision did not prepare and commit");
        return false;
    }
    let Some(third_slot) = ledger.test_native_slot(3, 1) else {
        eprintln!("bindless probe: third active revision had no native slot");
        return false;
    };
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let mut original_pixels = first_revision_pixels;
    original_pixels[third_slot as usize] = [15, 25, 35, 255];
    if adopted != [(3, 1)]
        || !check_table_probe(context, &ledger, LIBRARY, third_slot, original_pixels)
    {
        eprintln!("bindless probe: third key did not publish in its own table slot");
        return false;
    }

    // Keep the old table pinned while checking first and repeated resolution around publication.
    let old_ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("bindless probe: old epoch ticket failed: {error:?}");
            return false;
        }
    };
    let Some(old_view) = old_ticket.bindless() else {
        return false;
    };
    let old_id = match table_resource_id(old_view, first_slot) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("bindless probe: old table read failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            1,
            2,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![71, 81, 91, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 1, 2, &mut events)
        || ledger.commit(1, 2).is_err()
    {
        eprintln!("bindless probe: replacement did not prepare and commit");
        return false;
    }
    if ledger
        .offer(
            3,
            2,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![17, 27, 37, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 3, 2, &mut events)
        || ledger.commit(3, 2).is_err()
    {
        eprintln!("bindless probe: third-key replacement did not prepare and commit");
        return false;
    }
    let Some(replacement_slot) = ledger.test_native_slot(1, 2) else {
        eprintln!("bindless probe: replacement had no native slot");
        return false;
    };
    let Some(third_cached) = old_ticket.resolve_slot(3) else {
        eprintln!("bindless probe: third key did not resolve in the pinned table");
        return false;
    };
    if third_cached != (third_slot, 1) {
        eprintln!("bindless probe: third key resolved to the wrong pinned slot");
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    if !adopted.is_empty()
        || ledger.stats().committed_waiting_for_boundary != 2
        || table_resource_id(old_view, first_slot).ok() != Some(old_id)
    {
        eprintln!("bindless probe: adoption changed mappings while ticket resolution was open");
        return false;
    }
    let Some((first_after_attempt_slot, first_after_attempt_revision)) = old_ticket.resolve_slot(1)
    else {
        eprintln!("bindless probe: first resolution after blocked adoption failed");
        return false;
    };
    let Some(first_after_attempt) = old_ticket.resolve(1) else {
        eprintln!("bindless probe: first texture resolution after blocked adoption failed");
        return false;
    };
    if first_after_attempt_revision != 1
        || first_after_attempt_slot != first_slot
        || first_after_attempt.native_slot() != first_slot
        || table_resource_id(old_view, first_after_attempt_slot).ok() != Some(old_id)
    {
        eprintln!("bindless probe: first resolution did not match the ticket's pinned table");
        return false;
    }
    let mut frame_reader = match first_after_attempt.test_reader(Duration::ZERO, false) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("bindless probe: old revision reader creation failed: {error}");
            return false;
        }
    };
    drop(first_after_attempt);
    if old_ticket.resolve_slot(3) != Some((third_slot, 1))
        || old_ticket
            .resolve(3)
            .is_none_or(|texture| texture.native_slot() != third_slot)
    {
        eprintln!("bindless probe: repeated resolution changed across blocked adoption");
        return false;
    }

    ledger.retire_key(3);
    if old_ticket.resolve_slot(3) != Some((third_slot, 1))
        || old_ticket
            .resolve(3)
            .is_none_or(|texture| texture.native_slot() != third_slot)
    {
        eprintln!("bindless probe: table-pinned cached slot was lost after retirement");
        return false;
    }

    // Close resolution but keep the table version pinned. Build the probe on that old version,
    // publish the eligible replacement, then submit and read back while the ticket pin remains.
    if old_ticket.prepare_submit(&mut frame_reader).is_err() {
        eprintln!(
            "bindless probe: old epoch ticket did not close resolution at submit preparation"
        );
        return false;
    }
    let mut held_probe = match BindlessProbe::new(context, old_view, LIBRARY, true) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("bindless probe: held probe creation failed: {error}");
            return false;
        }
    };
    adopted.clear();
    ledger.adopt(2, &mut adopted);
    if adopted != [(1, 2)]
        || ledger.test_bindless_entries(first_slot).is_some()
        || table_resource_id(old_view, first_slot).ok() != Some(old_id)
        || table_resource_id(old_view, replacement_slot).ok() != Some(initial[0])
    {
        eprintln!(
            "bindless probe: publication did not wait for resolution close or changed the pinned table"
        );
        return false;
    }
    if held_probe.submit().is_err() {
        eprintln!("bindless probe: old epoch probe submission failed");
        return false;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !held_probe.gpu_done() && Instant::now() < deadline {
        thread::yield_now();
    }
    if !held_probe.gpu_done() || table_resource_id(old_view, first_slot).ok() != Some(old_id) {
        eprintln!("bindless probe: old epoch did not stay pinned through the GPU read");
        return false;
    }
    held_probe.open_gate();
    let deadline = Instant::now() + Duration::from_secs(5);
    let old_pixels = loop {
        if let Ok(pixels) = held_probe.readback() {
            break Some(pixels);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::yield_now();
    };
    // Negative control `pinned_write`: a write of the replacement into this pinned table would
    // make the held probe return the replacement pixels instead of the old revision.
    if old_pixels.as_deref() != Some(original_pixels.as_flattened()) {
        eprintln!("bindless probe: old epoch packet did not read its original pixels");
        return false;
    }
    if frame_reader.commit().is_err() || old_ticket.submitted().is_err() {
        eprintln!("bindless probe: ticket completion token submission failed");
        return false;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let frame_pixels = loop {
        ledger.pump(&mut events);
        if let Ok(pixels) = frame_reader.readback() {
            break Some(pixels);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::yield_now();
    };
    if frame_pixels.as_deref() != Some(&[31, 41, 51, 255][..]) {
        eprintln!("bindless probe: ticket completion did not retain the old resolved texture");
        return false;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while ledger
        .test_bindless_entries(first_slot)
        .is_none_or(|entries| entries != [initial[0], initial[0]])
        || ledger
            .test_bindless_entries(third_slot)
            .is_none_or(|entries| entries != [initial[0], initial[0]])
    {
        ledger.pump(&mut events);
        thread::yield_now();
        if Instant::now() >= deadline {
            break;
        }
    }
    if ledger
        .test_bindless_entries(first_slot)
        .is_none_or(|entries| entries != [initial[0], initial[0]])
        || ledger
            .test_bindless_entries(third_slot)
            .is_none_or(|entries| entries != [initial[0], initial[0]])
    {
        eprintln!(
            "bindless probe: retired old-table slots did not sanitize after the ticket ended: stats={:?}, releases_pending={}, first={:?}, third={:?}",
            ledger.stats(),
            ledger.has_release_work(),
            ledger.test_bindless_entries(1),
            ledger.test_bindless_entries(third_slot)
        );
        return false;
    }
    let epoch_ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("bindless probe: post-publication ticket failed: {error:?}");
            return false;
        }
    };
    if epoch_ticket
        .bindless()
        .is_none_or(|view| view.epoch != old_view.epoch + 1)
        || epoch_ticket.discarded().is_err()
    {
        eprintln!("bindless probe: epoch did not advance exactly once at the frame boundary");
        return false;
    }
    let mut replacement_pixels = [checker_pixel; 8];
    replacement_pixels[replacement_slot as usize] = [71, 81, 91, 255];
    if !check_table_probe(
        context,
        &ledger,
        LIBRARY,
        replacement_slot,
        replacement_pixels,
    ) {
        eprintln!("bindless probe: replacement table did not expose its texture");
        return false;
    }

    // Reject after spare preparation; both versions must return the candidate slot to the checker.
    if ledger
        .offer(
            2,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![121, 131, 141, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 2, 1, &mut events)
        || ledger.reject(2, 1).is_err()
    {
        eprintln!("bindless probe: rejection setup failed");
        return false;
    }
    let Some(rejected_slot) = ledger.test_native_slot(2, 1) else {
        eprintln!("bindless probe: rejected candidate had no native slot");
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while ledger.stats().retired != 0 && Instant::now() < deadline {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    if ledger.stats().retired != 0
        || ledger
            .test_bindless_entries(rejected_slot)
            .is_none_or(|entries| entries != [initial[0], initial[0]])
    {
        eprintln!("bindless probe: rejected spare slot was not sanitised in both tables");
        return false;
    }
    // No further scene edits occur after retirement; owner passes alone sanitize and release the
    // active replacement slot in both table versions.
    ledger.retire_key(1);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ledger.stats().retired != 0 && Instant::now() < deadline {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    if ledger.stats().retired != 0
        || ledger
            .test_bindless_entries(replacement_slot)
            .is_none_or(|entries| entries != [initial[0], initial[0]])
    {
        eprintln!("bindless probe: retirement did not sanitize without another scene edit");
        return false;
    }

    // Reuse a released slot so the shutdown probe runs beside a live ledger frame token.
    if ledger
        .offer(
            4,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![3, 13, 23, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 4, 1, &mut events)
        || ledger.commit(4, 1).is_err()
    {
        eprintln!("bindless probe: released slot did not prepare a new key");
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let Some(shutdown_slot) = ledger.test_native_slot(4, 1) else {
        eprintln!("bindless probe: new key has no native slot");
        return false;
    };

    // A delayed probe token makes the complete shutdown proof fail. Both the owner drop and
    // the later probe completion preserve the table, residency set and checker allocation.
    let shutdown_ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("bindless probe: shutdown ticket failed: {error:?}");
            return false;
        }
    };
    let Some(shutdown_view) = shutdown_ticket.bindless() else {
        return false;
    };
    let Some(texture) = shutdown_ticket.resolve(4) else {
        eprintln!("bindless probe: shutdown frame did not resolve its active revision");
        return false;
    };
    let mut reader = match texture.test_reader(Duration::from_secs(1), false) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("bindless probe: delayed reader creation failed: {error}");
            return false;
        }
    };
    drop(texture);
    let mut shutdown_probe = match BindlessProbe::new(context, shutdown_view, LIBRARY, true) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("bindless probe: shutdown probe creation failed: {error}");
            return false;
        }
    };
    if shutdown_probe.submit().is_err()
        || shutdown_ticket.prepare_submit(&mut reader).is_err()
        || reader.commit().is_err()
        || shutdown_ticket.submitted().is_err()
    {
        eprintln!("bindless probe: shutdown probe submission failed");
        return false;
    }
    let retained = context.native_live_objects();
    if !matches!(
        ledger.shutdown(Instant::now() + Duration::from_millis(10)),
        LedgerCleanup::Unproven { .. }
    ) || context.native_live_objects() != retained
    {
        eprintln!("bindless probe: delayed probe did not make shutdown retain all native objects");
        return false;
    }
    drop(ledger);
    if context.native_live_objects() != retained {
        eprintln!("bindless probe: dropping an unproven ledger released its residency state");
        return false;
    }
    shutdown_probe.open_gate();
    let deadline = Instant::now() + Duration::from_secs(5);
    let probe_pixels = loop {
        if let Ok(pixels) = shutdown_probe.readback() {
            break Some(pixels);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::yield_now();
    };
    let frame_pixels = loop {
        if let Ok(pixels) = reader.readback() {
            break Some(pixels);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::yield_now();
    };
    let mut shutdown_expected = [checker_pixel; 8];
    shutdown_expected[shutdown_slot as usize] = [3, 13, 23, 255];
    if probe_pixels.as_deref() != Some(shutdown_expected.as_flattened())
        || frame_pixels.as_deref() != Some(&[3, 13, 23, 255][..])
        || checker_ticket.discarded().is_err()
        || !matches!(
            provider.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!(
            "bindless probe: held shutdown probe failed to read the checker or provider proof failed"
        );
        return false;
    }
    true
}

#[derive(Clone, Copy)]
enum SpillSchedule {
    AllQueuedFirst,
    EarlySubmit,
}

fn spill_precondition(stats: &ck_gpu_metal::TextureStats) -> Result<(), String> {
    if stats.batches_prepared == 1
        && stats.batches_submitted == 1
        && stats.images_per_batch_max == 3
    {
        Ok(())
    } else {
        Err(format!(
            "precondition images_per_batch_max: expected 3, got {} (batches_prepared={}, batches_submitted={})",
            stats.images_per_batch_max, stats.batches_prepared, stats.batches_submitted
        ))
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the deterministic spill schedules verify bounded commits and their negative control"
)]
fn spill_check(context: &Arc<MetalContext>, schedule: SpillSchedule) -> Result<(), String> {
    // A small injected budget makes overflow deterministic in both debug and release profiles.
    // The production budget is 1,024; this exercises the same pump path with three ready uploads
    // and a test limit of two, so a debug_assert-wrapped take cannot pass unnoticed in release.
    let completion_gate = Arc::new(AtomicI32::new(0));
    let initially_released = matches!(schedule, SpillSchedule::EarlySubmit);
    let worker_gate = Arc::new((Mutex::new(initially_released), Condvar::new()));
    let ledger = TextureLedger::<u32>::start_bindless(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        BindlessLimits {
            texture_slots: 5,
            max_textures: 4,
            max_extra_revisions: 0,
        },
        128 * 1024 * 1024,
        0,
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            completion_gate: Some(Arc::clone(&completion_gate)),
            residency_change_budget: Some(2),
            block_worker: Some(Arc::clone(&worker_gate)),
            ..ck_gpu_metal::TextureTestHooks::default()
        },
    )
    .map_err(|error| format!("start failed: {error}"))?;
    let commits_before_spill = ledger.test_residency_commit_count().unwrap_or_default();

    let validation = match schedule {
        SpillSchedule::AllQueuedFirst => (|| {
            for key in 10..13 {
                offer_spill_item(&ledger, key)?;
            }
            open_spill_worker(&worker_gate);

            let mut ignored = Vec::new();
            let submit_deadline = Instant::now() + Duration::from_secs(5);
            while ledger.upload_stats().batches_submitted == 0 && Instant::now() < submit_deadline {
                ledger.pump(&mut ignored);
                thread::yield_now();
            }
            let before_completion = ledger.upload_stats();
            spill_precondition(&before_completion)?;

            completion_gate.store(1, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ledger.completion_arrived_unpolled() && Instant::now() < deadline {
                thread::yield_now();
            }
            if !ledger.completion_arrived_unpolled() {
                return Err("gated completion timed out".to_owned());
            }

            let mut prepared = Vec::new();
            let more = ledger.pump(&mut prepared);
            let first_pass = ledger.stats();
            if !more
                || prepared.len() != 2
                || first_pass.residency_changes_last_pass != 2
                || first_pass.residency_commits_last_pass != 1
                || ledger.test_residency_commit_count() != Some(commits_before_spill + 1)
            {
                return Err(format!(
                    "first pass expected two changes and one commit, events={}, stats={first_pass:?}",
                    prepared.len()
                ));
            }
            let more = ledger.pump(&mut prepared);
            let second_pass = ledger.stats();
            if more
                || prepared.len() != 3
                || second_pass.residency_changes_last_pass != 1
                || second_pass.residency_commits_last_pass != 1
                || ledger.test_residency_commit_count() != Some(commits_before_spill + 2)
            {
                return Err(format!(
                    "second pass expected one remaining change and one commit, events={}, stats={second_pass:?}",
                    prepared.len()
                ));
            }
            Ok(())
        })(),
        SpillSchedule::EarlySubmit => (|| {
            offer_spill_item(&ledger, 10)?;
            let mut ignored = Vec::new();
            let submit_deadline = Instant::now() + Duration::from_secs(5);
            while ledger.upload_stats().batches_submitted == 0 && Instant::now() < submit_deadline {
                ledger.pump(&mut ignored);
                thread::yield_now();
            }
            let first_batch = ledger.upload_stats();
            if first_batch.batches_prepared != 1
                || first_batch.batches_submitted != 1
                || first_batch.images_per_batch_max != 1
            {
                return Err(format!(
                    "early-submit control did not establish a one-image first batch: {first_batch:?}"
                ));
            }
            offer_spill_item(&ledger, 11)?;
            offer_spill_item(&ledger, 12)?;
            spill_precondition(&first_batch)
        })(),
    };

    // Every exit path releases both test gates before asking the ledger for its proof.
    completion_gate.store(1, Ordering::SeqCst);
    open_spill_worker(&worker_gate);
    match ledger.shutdown(Instant::now() + Duration::from_secs(5)) {
        LedgerCleanup::Proven => validation,
        LedgerCleanup::Unproven { reason } => Err(format!(
            "cleanup was unproven after {validation:?}: {reason}"
        )),
    }
}

fn offer_spill_item(ledger: &TextureLedger<u32>, key: u32) -> Result<(), String> {
    ledger
        .offer(
            key,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![u8::try_from(key).expect("small key"), 2, 3, 255],
                drops: None,
            }),
        )
        .map_err(|_| format!("offer {key} was refused"))
}

fn open_spill_worker(gate: &Arc<(Mutex<bool>, Condvar)>) {
    let (mutex, changed) = &**gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
}

fn check_table_probe(
    context: &Arc<MetalContext>,
    ledger: &TextureLedger<u32>,
    library: &[u8],
    slot: u32,
    expected: [[u8; 4]; 8],
) -> bool {
    let ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("bindless probe: frame ticket failed: {error:?}");
            return false;
        }
    };
    let Some(view) = ticket.bindless() else {
        eprintln!("bindless probe: frame ticket has no table view");
        return false;
    };
    let mut command = match BindlessProbe::new(context, view, library, false) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("bindless probe: command creation failed: {error}");
            return false;
        }
    };
    if command.submit().is_err() {
        eprintln!("bindless probe: command submission failed");
        return false;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let pixels = loop {
        if let Ok(pixels) = command.readback() {
            break Some(pixels);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::yield_now();
    };
    if pixels.as_deref() != Some(expected.as_flattened()) || ticket.discarded().is_err() {
        eprintln!("bindless probe: table slot {slot} did not sample expected pixels");
        return false;
    }
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the charge-cap regression checks admission, retire-many and returned capacity"
)]
fn verify_ledger_charge_cap(context: &Arc<MetalContext>) -> bool {
    let limits = LedgerLimits {
        max_extra_revisions: 2,
        max_adoptions_per_frame: 256,
    };
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        limits,
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger cap: start failed: {error}");
            return false;
        }
    };
    let offer = |key, revision, byte| {
        ledger.offer(
            key,
            revision,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![byte, byte, byte, 255],
                drops: None,
            }),
        )
    };
    for (revision, byte) in [(1_u64, 1_u8), (2, 2), (3, 3)] {
        if offer(1, revision, byte).is_err() {
            eprintln!("ledger cap: baseline plus two extras were not admitted");
            return false;
        }
    }
    if ledger.extra_room() != 0 || !matches!(offer(1, 4, 4), Err(OfferRefusal::Protocol(_))) {
        eprintln!("ledger cap: full extra cap did not report protocol refusal");
        return false;
    }
    if offer(2, 1, 20).is_err()
        || ledger.stats().baseline_charges != 2
        || ledger.stats().extra_revisions != 2
    {
        eprintln!("ledger cap: baseline-only first load was refused at a full extra cap");
        return false;
    }
    // More baseline-only keys than the extra cap must remain admissible and retire without
    // consuming replacement capacity. Negative control: treating every Retired revision as an
    // extra would make these three keys exceed the cap of two.
    for (key, byte) in [(3, 23), (4, 24)] {
        if offer(key, 1, byte).is_err()
            || ledger.stats().baseline_charges + ledger.stats().extra_revisions
                != ledger.stats().in_flight
                    + ledger.stats().prepared
                    + ledger.stats().committed_waiting_for_boundary
                    + ledger.stats().active
                    + ledger.stats().retired
        {
            eprintln!(
                "ledger cap negative control: baseline-only key {key} violated charge conservation: {:?}",
                ledger.stats()
            );
            return false;
        }
    }
    let mut events = Vec::new();
    let mut adopted = Vec::new();
    for key in [2, 3, 4] {
        let prepared = wait_ledger_prepared(context, &ledger, key, 1, &mut events);
        let committed = prepared && ledger.commit(key, 1).is_ok();
        if !committed {
            eprintln!(
                "ledger cap: baseline-only key {key} did not prepare and commit: prepared={prepared}, stats={:?}, events={events:?}",
                ledger.stats()
            );
            return false;
        }
    }
    ledger.adopt(3, &mut adopted);
    let held = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger cap: retire-many pin ticket failed: {error:?}");
            return false;
        }
    };
    for key in [2, 3, 4] {
        if held.resolve(key).is_none() {
            eprintln!("ledger cap: baseline-only key {key} did not become active");
            return false;
        }
    }
    for key in [2, 3, 4] {
        ledger.retire_key(key);
    }
    if ledger.stats().retired != 3 || ledger.stats().extra_revisions != 2 {
        eprintln!(
            "ledger cap negative control: pinned baseline retire-many changed extra charges: {:?}",
            ledger.stats()
        );
        return false;
    }
    // Negative control: counting all three Retired baseline revisions as extras would exceed cap 2.
    if held.discarded().is_err() {
        eprintln!("ledger cap: pinned baseline retire-many ticket did not discard");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(5);
    while ![2, 3, 4].into_iter().all(|key| {
        events
            .iter()
            .any(|event| matches!(event, LedgerEvent::KeyRetired { key: found } if *found == key))
    }) {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger cap: baseline-only retire-many case stalled");
            return false;
        }
        thread::yield_now();
    }
    if ledger.stats().extra_revisions != 2
        || ledger.stats().baseline_charges != 1
        || ledger.stats().retired != 0
    {
        eprintln!("ledger cap: retirement created or consumed an extra charge");
        return false;
    }
    ledger.retire_key(1);
    events.clear();
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 1 }))
    {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger cap: replacement capacity did not return on retirement");
            return false;
        }
        thread::yield_now();
    }
    if ledger.extra_room() != 2
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!("ledger cap: charge cleanup or shutdown proof failed");
        return false;
    }
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the review regressions exercise independent retirement, ticket and proof contracts"
)]
fn verify_ledger_review_regressions(context: &Arc<MetalContext>) -> bool {
    // B2: a new frame cannot resolve a retired active revision; an older exact pin keeps it alive
    // only until explicit discard. Negative control: resolving after retirement would add a pin.
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger review: retirement ledger start failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            1,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![1, 2, 3, 255],
                drops: None,
            }),
        )
        .is_err()
    {
        return false;
    }
    let mut events = Vec::new();
    if !wait_ledger_prepared(context, &ledger, 1, 1, &mut events) || ledger.commit(1, 1).is_err() {
        eprintln!("ledger review: active baseline did not prepare");
        return false;
    }
    let mut adopted = Vec::new();
    ledger.adopt(1, &mut adopted);
    let (mut held_reader, held) =
        match begin_ledger_reader(&ledger, 1, Duration::from_millis(150), false) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("ledger review: pre-retirement pin setup failed: {error}");
                return false;
            }
        };
    if held.prepare_submit(&mut held_reader).is_err()
        || held_reader.commit().is_err()
        || held.submitted().is_err()
    {
        eprintln!("ledger review: pre-retirement frame submission failed");
        return false;
    }
    ledger.retire_key(1);
    let after_retirement = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger review: new ticket after retirement failed: {error:?}");
            return false;
        }
    };
    if after_retirement.resolve(1).is_some()
        || ledger.stats().live_pins != 1
        || ledger.stats().retired != 1
    {
        eprintln!(
            "ledger review negative control: retired active revision was resolvable or lost its old pin"
        );
        return false;
    }
    if after_retirement.discarded().is_err() {
        eprintln!("ledger review: post-retirement empty ticket discard failed");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(2);
    while ledger.stats().tickets_submitted_pending != 0 && Instant::now() < until {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    ledger.pump(&mut events);
    if !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 1 }))
        || ledger.stats().retired != 0
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!("ledger review: existing pin did not release retired revision on discard");
        return false;
    }

    // B3: one shutdown call waits for a short-lived frame token under its absolute deadline.
    // Negative control: the old immediate-refusal behavior returns Unproven before the token ends.
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger review: bounded-proof ledger start failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            2,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![4, 5, 6, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 2, 1, &mut events)
        || ledger.commit(2, 1).is_err()
    {
        eprintln!("ledger review: bounded-proof upload setup failed");
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let (mut reader, ticket) =
        match begin_ledger_reader(&ledger, 2, Duration::from_millis(80), false) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("ledger review: bounded-proof reader setup failed: {error}");
                return false;
            }
        };
    if ticket.prepare_submit(&mut reader).is_err()
        || reader.commit().is_err()
        || ticket.submitted().is_err()
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_secs(2)),
            LedgerCleanup::Proven
        )
    {
        eprintln!(
            "ledger review negative control: one shutdown did not prove a token completing inside its deadline"
        );
        return false;
    }
    if reader.readback().is_err() || ledger.stats().active + ledger.stats().retired != 0 {
        eprintln!("ledger review: successful proof did not end the frame and release the ledger");
        return false;
    }

    // B4 ticket cases: an unsubmitted ticket remains a live pin across prove and retirement, then
    // its explicit discard releases it. Negative control: dropping it would latch uncertainty.
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger review: held-ticket ledger start failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            3,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![7, 8, 9, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 3, 1, &mut events)
        || ledger.commit(3, 1).is_err()
    {
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let held = match ledger.begin_frame() {
        Ok(ticket) if ticket.resolve(3).is_some() => ticket,
        _ => return false,
    };
    let native_before = context.native_live_objects();
    if ledger
        .prove(Instant::now() + Duration::from_millis(20))
        .is_complete()
    {
        eprintln!("ledger review: live unsubmitted ticket was proven");
        return false;
    }
    ledger.retire_key(3);
    if ledger.stats().live_pins != 1 || context.native_live_objects() != native_before {
        eprintln!(
            "ledger review negative control: live unsubmitted pin failed to retain its retired texture"
        );
        return false;
    }
    if held.discarded().is_err() {
        eprintln!("ledger review: live unsubmitted ticket discard failed");
        return false;
    }
    ledger.pump(&mut events);
    if !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 3 }))
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!("ledger review: discarded unsubmitted pin did not complete retirement");
        return false;
    }

    // A dropped submitting ticket, not only a building ticket, must conservatively latch failure.
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger review: submitting-ticket ledger start failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            4,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![10, 11, 12, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 4, 1, &mut events)
        || ledger.commit(4, 1).is_err()
    {
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let (mut command, ticket) = match begin_ledger_reader(&ledger, 4, Duration::ZERO, false) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("ledger review: submitting-ticket reader setup failed: {error}");
            return false;
        }
    };
    if ticket.prepare_submit(&mut command).is_err() {
        eprintln!("ledger review: ticket did not enter submitting phase");
        return false;
    }
    drop(ticket);
    ledger.pump(&mut events);
    if ledger.stats().tickets_uncertain != 1
        || !events
            .iter()
            .any(|event| matches!(event, LedgerEvent::GpuFailure { .. }))
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_millis(10)),
            LedgerCleanup::Unproven { .. }
        )
    {
        eprintln!(
            "ledger review negative control: dropped submitting ticket did not latch uncertainty"
        );
        return false;
    }
    drop(command);

    // The room wake follows returned capacity, allowing another take with no new edit. A rejected
    // retiring-key request returns its CPU bytes. Negative control: losing either wake or payload
    // leaves the producer stalled or leaks the source.
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake_count = Arc::clone(&wakes);
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits {
            max_prepared_images: 2,
            max_upload_images_per_batch: 1,
            ..TextureLimits::default()
        },
        LedgerLimits {
            max_extra_revisions: 1,
            ..LedgerLimits::default()
        },
        TextureWakes {
            room: Some(Arc::new(move || {
                wake_count.fetch_add(1, Ordering::SeqCst);
            })),
            ..TextureWakes::default()
        },
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger review: room-wake ledger start failed: {error}");
            return false;
        }
    };
    let drops = Arc::new(AtomicUsize::new(0));
    let payload = Arc::new(Pixels {
        bytes: vec![13, 14, 15, 255],
        drops: Some(Arc::clone(&drops)),
    });
    if ledger.offer(5, 1, 1, 1, payload).is_err() {
        return false;
    }
    let payload = Arc::new(Pixels {
        bytes: vec![16, 17, 18, 255],
        drops: Some(Arc::clone(&drops)),
    });
    if ledger.offer(5, 2, 1, 1, payload).is_err() || ledger.extra_room() != 0 {
        eprintln!("ledger review: replacement cap did not fill");
        return false;
    }
    let rejected = Arc::new(Pixels {
        bytes: vec![19, 20, 21, 255],
        drops: Some(Arc::clone(&drops)),
    });
    if !matches!(
        ledger.offer(5, 3, 1, 1, rejected),
        Err(OfferRefusal::Protocol(_))
    ) || drops.load(Ordering::SeqCst) != 1
    {
        eprintln!(
            "ledger review negative control: full extra cap took the request or kept its CPU bytes"
        );
        return false;
    }
    let retiring_drops = Arc::new(AtomicUsize::new(0));
    ledger.retire_key(5);
    if !matches!(
        ledger.offer(
            5,
            4,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![22, 23, 24, 255],
                drops: Some(Arc::clone(&retiring_drops))
            })
        ),
        Err(OfferRefusal::KeyRetiring)
    ) || retiring_drops.load(Ordering::SeqCst) != 1
    {
        eprintln!(
            "ledger review negative control: retiring-key refusal retained request CPU bytes"
        );
        return false;
    }
    events.clear();
    let until = Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 5 }))
    {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger review: room-wake retirement timed out");
            return false;
        }
        thread::yield_now();
    }
    if wakes.load(Ordering::SeqCst) == 0 || ledger.extra_room() != 1 {
        eprintln!("ledger review negative control: returned capacity did not wake the producer");
        return false;
    }
    if ledger
        .offer(
            6,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![25, 26, 27, 255],
                drops: None,
            }),
        )
        .is_err()
        || !matches!(
            ledger.shutdown(Instant::now() + Duration::from_secs(3)),
            LedgerCleanup::Proven
        )
    {
        eprintln!(
            "ledger review: producer could not take after the capacity wake without a new edit"
        );
        return false;
    }

    // Upload completion must not end a still-delayed frame pin. Negative control: conflating the
    // upload and frame tokens releases the retiring key before the reader has completed.
    let ledger = match TextureLedger::<u32>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits::default(),
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger review: split-token ledger start failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            7,
            1,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![31, 32, 33, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 7, 1, &mut events)
        || ledger.commit(7, 1).is_err()
    {
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    let (mut reader, ticket) = match begin_ledger_reader(&ledger, 7, Duration::from_secs(5), false)
    {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("ledger review: delayed frame reader setup failed: {error}");
            return false;
        }
    };
    if ledger
        .offer(
            7,
            2,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![34, 35, 36, 255],
                drops: None,
            }),
        )
        .is_err()
        || !wait_ledger_prepared(context, &ledger, 7, 2, &mut events)
        || ledger.stats().live_pins != 1
        || ledger.stats().in_flight != 0
    {
        eprintln!("ledger review negative control: upload completion ended the building frame pin");
        return false;
    }
    if ledger.commit(7, 2).is_err() {
        eprintln!("ledger review: completed replacement did not commit");
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    if !adopted.is_empty()
        || !ledger.has_committed_or_active(7)
        || ledger.stats().live_pins != 1
        || ledger.stats().retired != 0
    {
        eprintln!("ledger review: open ticket did not defer replacement adoption");
        return false;
    }
    if ticket.prepare_submit(&mut reader).is_err() {
        eprintln!("ledger review: delayed frame ticket could not submit");
        return false;
    }
    adopted.clear();
    ledger.adopt(1, &mut adopted);
    if adopted != [(7, 2)] || ledger.stats().live_pins != 1 || ledger.stats().retired != 1 {
        eprintln!("ledger review: closed ticket did not adopt replacement with old pin retained");
        return false;
    }
    if reader.commit().is_err() || ticket.submitted().is_err() {
        eprintln!("ledger review: delayed frame token submission failed");
        return false;
    }
    events.clear();
    ledger.retire_key(7);
    ledger.pump(&mut events);
    if ledger.stats().tickets_submitted_pending != 1
        || ledger.stats().live_pins != 1
        || ledger.stats().retired == 0
        || events
            .iter()
            .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 7 }))
    {
        eprintln!("ledger review negative control: upload completion ended the pending frame pin");
        return false;
    }
    let until = Instant::now() + Duration::from_secs(7);
    while ledger.stats().tickets_submitted_pending != 0 && Instant::now() < until {
        ledger.pump(&mut events);
        thread::yield_now();
    }
    while !events
        .iter()
        .any(|event| matches!(event, LedgerEvent::KeyRetired { key: 7 }))
    {
        ledger.pump(&mut events);
        if Instant::now() >= until {
            eprintln!("ledger review: frame completion did not finish retirement");
            return false;
        }
        thread::yield_now();
    }
    if !matches!(
        ledger.shutdown(Instant::now() + Duration::from_secs(3)),
        LedgerCleanup::Proven
    ) {
        eprintln!("ledger review: split-token cleanup proof failed");
        return false;
    }
    true
}

/// A key that counts how often it is compared, so the cost of resolving many keys in one ticket is measured in
/// comparisons, not seconds.
static KEY_COMPARISONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug)]
struct CountedKey(u32);

impl std::hash::Hash for CountedKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for CountedKey {
    fn eq(&self, other: &Self) -> bool {
        KEY_COMPARISONS.fetch_add(1, Ordering::Relaxed);
        self.0 == other.0
    }
}

impl Eq for CountedKey {}

/// CK-113's defect found by CK-116: `FrameTicket::resolve` scanned the ticket's pins once per distinct key, so resolving
/// a frame's `n` textures cost O(n^2) key comparisons. With `n` distinct shown textures resolved twice each in one ticket
/// the comparisons must stay linear (a small constant per resolve), and each revision is pinned once.
fn verify_ledger_resolves_many_keys_linearly(context: &Arc<MetalContext>) -> bool {
    const KEYS: u32 = 1500;
    let ledger = match TextureLedger::<CountedKey>::start(
        Arc::clone(context),
        TextureLimits::default(),
        LedgerLimits {
            max_extra_revisions: 16,
            max_adoptions_per_frame: 4096,
        },
        TextureWakes::default(),
    ) {
        Ok(ledger) => ledger,
        Err(error) => {
            eprintln!("ledger scale: start failed: {error}");
            return false;
        }
    };
    let until = Instant::now() + Duration::from_secs(40);
    let mut events = Vec::new();
    let (mut next, mut committed) = (0_u32, 0_u32);
    while committed < KEYS {
        while next < KEYS && ledger.room() > 0 {
            let pixels = Arc::new(Pixels {
                bytes: vec![1, 2, 3, 255],
                drops: None,
            });
            if ledger.offer(CountedKey(next), 1, 1, 1, pixels).is_err() {
                break;
            }
            next += 1;
        }
        events.clear();
        ledger.pump(&mut events);
        for event in &events {
            if let LedgerEvent::Prepared { key, revision } = event {
                if ledger.commit(*key, *revision).is_err() {
                    eprintln!("ledger scale: commit refused");
                    return false;
                }
                committed += 1;
            }
        }
        if Instant::now() >= until {
            eprintln!("ledger scale: only {committed} of {KEYS} revisions committed in time");
            return false;
        }
        thread::yield_now();
    }
    let mut adopted = Vec::new();
    ledger.adopt(4096, &mut adopted);
    if adopted.len() != KEYS as usize {
        eprintln!("ledger scale: {} of {KEYS} adopted", adopted.len());
        return false;
    }
    let ticket = match ledger.begin_frame() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("ledger scale: begin_frame: {error:?}");
            return false;
        }
    };
    KEY_COMPARISONS.store(0, Ordering::Relaxed);
    for _ in 0..2 {
        for key in 0..KEYS {
            if ticket.resolve(CountedKey(key)).is_none() {
                eprintln!("ledger scale: key {key} did not resolve");
                return false;
            }
        }
    }
    let comparisons = KEY_COMPARISONS.load(Ordering::Relaxed);
    let pins = ledger.stats().live_pins;
    if ticket.discarded().is_err() {
        eprintln!("ledger scale: the ticket could not be discarded");
        return false;
    }
    // A scan of the pins compares about KEYS * KEYS / 2 = 1.1 million times; a hashed membership test a few per resolve.
    let bound = 24 * 2 * KEYS as usize;
    if pins != KEYS as usize || comparisons > bound {
        eprintln!(
            "ledger scale: {pins} live pins (want {KEYS}); {comparisons} key comparisons (bound {bound})"
        );
        return false;
    }
    println!(
        "ledger scale: {KEYS} distinct keys resolved twice in one ticket with {comparisons} key comparisons"
    );
    true
}

/// Why a spare-map case did not pass: `Setup` is a protocol or native failure before the thing under test was exercised; `Assertion` is
/// the thing under test behaving other than required. A negative control counts only when it fails with an `Assertion`.
#[derive(Debug)]
enum CaseError {
    Setup(String),
    Assertion(String),
}

impl From<String> for CaseError {
    fn from(message: String) -> Self {
        Self::Setup(message)
    }
}

impl From<&str> for CaseError {
    fn from(message: &str) -> Self {
        Self::Setup(message.to_owned())
    }
}

impl std::fmt::Display for CaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(message) => write!(f, "setup: {message}"),
            Self::Assertion(message) => write!(f, "assertion: {message}"),
        }
    }
}

fn assertion<T>(message: String) -> Result<T, CaseError> {
    Err(CaseError::Assertion(message))
}

/// Judges a negative control: it must fail with an assertion (not a setup error). Prints the reason it failed.
fn control_failed(label: &str, outcome: Result<(), CaseError>) -> bool {
    match outcome {
        Err(CaseError::Assertion(reason)) => {
            println!("spare control {label}: failed as required: {reason}");
            true
        }
        Err(CaseError::Setup(reason)) => {
            eprintln!("spare control {label}: failed in setup, not as required: {reason}");
            false
        }
        Ok(()) => {
            eprintln!("spare control {label}: passed, so it shows nothing");
            false
        }
    }
}

/// Which way a spare-map check ends its first ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpareEnding {
    Discarded,
    Submitted,
    NotCommitted,
}

/// A ledger for the ticket-map reuse checks: ordinary (handles) or bindless (slots), with `fault` switched on when given.
fn spare_ledger(
    context: &Arc<MetalContext>,
    bindless: bool,
    fault: Option<ck_gpu_metal::SpareFault>,
) -> Result<TextureLedger<u32>, String> {
    let limits = LedgerLimits {
        max_extra_revisions: 8,
        max_adoptions_per_frame: 16,
    };
    let ledger = if bindless {
        TextureLedger::<u32>::start_bindless(
            Arc::clone(context),
            TextureLimits::default(),
            limits,
            BindlessLimits {
                texture_slots: 16,
                max_textures: 8,
                max_extra_revisions: 4,
            },
            128 * 1024 * 1024,
            0,
            TextureWakes::default(),
            ck_gpu_metal::TextureTestHooks::default(),
        )
    } else {
        TextureLedger::<u32>::start(
            Arc::clone(context),
            TextureLimits::default(),
            limits,
            TextureWakes::default(),
        )
    }
    .map_err(|error| format!("start: {error}"))?;
    if let Some(fault) = fault {
        ledger.test_spare_fault(fault);
    }
    Ok(ledger)
}

/// Uploads, commits and adopts one revision of `key`.
fn spare_upload(
    ledger: &TextureLedger<u32>,
    key: u32,
    revision: u64,
    color: u8,
) -> Result<(), String> {
    ledger
        .offer(
            key,
            revision,
            1,
            1,
            Arc::new(Pixels {
                bytes: vec![color, color, color, 255],
                drops: None,
            }),
        )
        .map_err(|error| format!("offer {key}/{revision}: {error:?}"))?;
    let until = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        ledger.pump(&mut events);
        if events.iter().any(|event| matches!(event, LedgerEvent::Prepared { key: found, revision: found_revision } if *found == key && *found_revision == revision)) {
            break;
        }
        if Instant::now() >= until {
            return Err(format!("{key}/{revision} was not prepared in time"));
        }
        thread::yield_now();
    }
    ledger
        .commit(key, revision)
        .map_err(|error| format!("commit {key}/{revision}: {error:?}"))?;
    let mut adopted = Vec::new();
    ledger.adopt(16, &mut adopted);
    if adopted.contains(&(key, revision)) {
        Ok(())
    } else {
        Err(format!("{key}/{revision} was not adopted: {adopted:?}"))
    }
}

/// Resolves `key` as the build does (the bindless slot path or the ordinary pinning path): whether it resolved, and on the bindless
/// path the revision it resolved to.
fn spare_resolve(
    ticket: &ck_gpu_metal::FrameTicket<'_, u32>,
    bindless: bool,
    key: u32,
) -> (bool, Option<u64>) {
    if bindless {
        match ticket.resolve_slot(key) {
            Some((_, revision)) => (true, Some(revision)),
            None => (false, None),
        }
    } else {
        (ticket.resolve(key).is_some(), None)
    }
}

/// Ends `ticket` (which resolved key 1 through `resolve`) the way `ending` says.
fn end_spare_ticket(
    ticket: ck_gpu_metal::FrameTicket<'_, u32>,
    ending: SpareEnding,
) -> Result<(), String> {
    if ending == SpareEnding::Discarded {
        return ticket
            .discarded()
            .map_err(|error| format!("discarded: {error:?}"));
    }
    let mut command = {
        let texture = ticket.resolve(1).ok_or("no texture to read")?;
        texture
            .test_reader(Duration::ZERO, false)
            .map_err(|error| error.to_string())?
    };
    ticket
        .prepare_submit(&mut command)
        .map_err(|error| format!("prepare_submit: {error:?}"))?;
    if ending == SpareEnding::Submitted {
        if command.commit().is_err() {
            return Err("the command buffer did not commit".to_owned());
        }
        ticket
            .submitted()
            .map_err(|error| format!("submitted: {error:?}"))
    } else {
        ticket
            .not_committed()
            .map_err(|error| format!("not_committed: {error:?}"))
    }
}

/// Frame 1 resolves keys 1 and 2 and ends; key 2 then adopts a new revision; frame 2 must resolve key 2 to it. With `fault` set the
/// case must fail (its negative control).
fn spare_reuse_case(
    context: &Arc<MetalContext>,
    bindless: bool,
    ending: SpareEnding,
    fault: Option<ck_gpu_metal::SpareFault>,
) -> Result<(), CaseError> {
    let ledger = spare_ledger(context, bindless, fault)?;
    ledger
        .set_spare_limit(8)
        .map_err(|error| format!("set_spare_limit: {error:?}"))?;
    spare_upload(&ledger, 1, 1, 10)?;
    spare_upload(&ledger, 2, 1, 20)?;
    let first = ledger
        .begin_frame()
        .map_err(|e| format!("begin 1: {e:?}"))?;
    for key in [1, 2] {
        if !spare_resolve(&first, bindless, key).0 {
            return Err(CaseError::Setup(format!(
                "frame 1 could not resolve key {key}"
            )));
        }
    }
    end_spare_ticket(first, ending)?;
    let stats = ledger.stats();
    if stats.spare_ticket_entries != 0 || stats.spare_ticket_capacity < 2 {
        return assertion(format!(
            "after {ending:?} the spares hold {} entries with capacity {}",
            stats.spare_ticket_entries, stats.spare_ticket_capacity
        ));
    }
    spare_upload(&ledger, 2, 2, 30)?;
    let second = ledger
        .begin_frame()
        .map_err(|e| format!("begin 2: {e:?}"))?;
    let (resolved, revision) = spare_resolve(&second, bindless, 2);
    if !resolved || (bindless && revision != Some(2)) {
        return assertion(format!(
            "frame 2 resolved key 2 to {revision:?} (resolved: {resolved}), not its new revision"
        ));
    }
    second
        .discarded()
        .map_err(|error| format!("discarded 2: {error:?}"))?;
    spare_shutdown(&ledger);
    Ok(())
}

/// Frame 1 resolves keys 1 and 2 and is dropped without an ending: the maps came back cleared and the failure is preserved (no
/// frame 2: the ledger is ended and the check does not reopen it).
fn spare_dropped_case(
    context: &Arc<MetalContext>,
    bindless: bool,
    fault: Option<ck_gpu_metal::SpareFault>,
) -> Result<(), CaseError> {
    let ledger = spare_ledger(context, bindless, fault)?;
    ledger
        .set_spare_limit(8)
        .map_err(|error| format!("set_spare_limit: {error:?}"))?;
    spare_upload(&ledger, 1, 1, 10)?;
    spare_upload(&ledger, 2, 1, 20)?;
    let first = ledger
        .begin_frame()
        .map_err(|e| format!("begin 1: {e:?}"))?;
    for key in [1, 2] {
        if !spare_resolve(&first, bindless, key).0 {
            return Err(CaseError::Setup(format!(
                "frame 1 could not resolve key {key}"
            )));
        }
    }
    drop(first);
    let stats = ledger.stats();
    if stats.spare_ticket_entries != 0 || stats.spare_ticket_capacity < 2 {
        return assertion(format!(
            "after a drop the spares hold {} entries with capacity {}",
            stats.spare_ticket_entries, stats.spare_ticket_capacity
        ));
    }
    if stats.tickets_uncertain != 1 {
        return assertion(format!(
            "{} uncertain tickets, want 1",
            stats.tickets_uncertain
        ));
    }
    let mut events = Vec::new();
    ledger.pump(&mut events);
    let failures = events
        .iter()
        .filter(|event| matches!(event, LedgerEvent::GpuFailure { message } if message.contains("frame ticket dropped without an explicit outcome")))
        .count();
    if failures != 1 {
        return assertion(format!("{failures} drop failures in {events:?}, want 1"));
    }
    match ledger.begin_frame() {
        Err(ck_gpu_metal::LedgerError::Ended) => Ok(()),
        Err(error) => assertion(format!("begin_frame after the drop: {error:?}")),
        Ok(_) => assertion("begin_frame after the drop was accepted".to_owned()),
    }
}

/// Proves and releases a ledger a check is done with (its outcome is not the check's).
fn spare_shutdown(ledger: &TextureLedger<u32>) {
    let _ = ledger.shutdown(Instant::now() + Duration::from_secs(5));
}

/// The capacity the standard map allocates for `entries`.
fn capacity_for(entries: usize) -> usize {
    std::collections::HashMap::<u32, Option<u64>>::with_capacity(entries).capacity()
}

/// Opens a ticket on `ledger`, resolves `count` keys it does not know (each caches `None`) and discards it.
fn spare_unknown_frame(ledger: &TextureLedger<u32>, first: u32, count: u32) -> Result<(), String> {
    let ticket = ledger
        .begin_frame()
        .map_err(|error| format!("begin: {error:?}"))?;
    for key in first..first + count {
        if ticket.resolve(key).is_some() {
            return Err(format!("unknown key {key} resolved"));
        }
    }
    ticket
        .discarded()
        .map_err(|error| format!("discarded: {error:?}"))
}

fn verify_spare_ticket_maps(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::SpareFault::{ExplicitOnly, SkipClear};
    for bindless in [false, true] {
        let path = if bindless { "bindless" } else { "ordinary" };
        for ending in [
            SpareEnding::Discarded,
            SpareEnding::Submitted,
            SpareEnding::NotCommitted,
        ] {
            if let Err(error) = spare_reuse_case(context, bindless, ending, None) {
                eprintln!("spare maps ({path}, {ending:?}): {error}");
                return false;
            }
            let label = format!("{path}/{ending:?}/SkipClear");
            if !control_failed(
                &label,
                spare_reuse_case(context, bindless, ending, Some(SkipClear)),
            ) {
                return false;
            }
        }
        if let Err(error) = spare_dropped_case(context, bindless, None) {
            eprintln!("spare maps ({path}, dropped): {error}");
            return false;
        }
        for fault in [SkipClear, ExplicitOnly] {
            let label = format!("{path}/dropped/{fault:?}");
            if !control_failed(&label, spare_dropped_case(context, bindless, Some(fault))) {
                return false;
            }
        }
    }
    println!("spare maps: reuse is cleared on every ending and its controls fail");
    true
}

fn spare_oversized_case(
    context: &Arc<MetalContext>,
    fault: Option<ck_gpu_metal::SpareFault>,
) -> Result<(), CaseError> {
    let ledger = spare_ledger(context, false, fault)?;
    ledger
        .set_spare_limit(4)
        .map_err(|error| format!("set_spare_limit: {error:?}"))?;
    spare_unknown_frame(&ledger, 100, 10)?;
    let oversized = ledger.stats().spare_ticket_capacity;
    if oversized != 0 {
        return assertion(format!("an oversized map was kept (capacity {oversized})"));
    }
    spare_unknown_frame(&ledger, 200, 2)?;
    let kept = ledger.stats().spare_ticket_capacity;
    if kept < 2 || kept > capacity_for(4) {
        return assertion(format!(
            "the small map's kept capacity {kept} is not within 2..={}",
            capacity_for(4)
        ));
    }
    // No limit set: nothing is kept after a frame that resolved any key.
    let unset = spare_ledger(context, false, None)?;
    spare_unknown_frame(&unset, 300, 3)?;
    let unset_capacity = unset.stats().spare_ticket_capacity;
    if unset_capacity != 0 {
        return assertion(format!("an unset ledger kept capacity {unset_capacity}"));
    }
    spare_shutdown(&ledger);
    spare_shutdown(&unset);
    Ok(())
}

fn verify_spare_oversized_map_is_not_kept(context: &Arc<MetalContext>) -> bool {
    if let Err(error) = spare_oversized_case(context, None) {
        eprintln!("spare oversized: {error}");
        return false;
    }
    if !control_failed(
        "oversized/KeepOversized",
        spare_oversized_case(context, Some(ck_gpu_metal::SpareFault::KeepOversized)),
    ) {
        return false;
    }
    println!(
        "spare oversized: a map over the limit is dropped, a small one kept, and the control fails"
    );
    true
}

/// What the limit-fixed observation saw after the first setting (1,000) and a second attempt (4).
#[derive(Debug)]
struct SpareLimitSeen {
    /// The second `set_spare_limit(4)` was refused with `SpareLimitFixed`.
    second_refused: bool,
    /// The kept capacity after a frame of 2 unknown keys (within the lowered bound of 4) that took the warmed spare.
    small_frame_capacity: usize,
    /// The kept capacity after a frame of 10 unknown keys (over 4, within 1,000).
    ten_key_capacity: usize,
}

/// Sets the limit to 1,000, warms a spare with a 100-key frame, then tries to lower it to 4 and observes what the next frames keep.
/// An `Err` is a setup failure, never an outcome of the thing under test.
fn spare_limit_observe(
    context: &Arc<MetalContext>,
    fault: Option<ck_gpu_metal::SpareFault>,
) -> Result<SpareLimitSeen, String> {
    use ck_gpu_metal::LedgerError::SpareLimitFixed;
    let ledger = spare_ledger(context, false, fault)?;
    ledger
        .set_spare_limit(1000)
        .map_err(|error| format!("the first setting failed: {error:?}"))?;
    spare_unknown_frame(&ledger, 100, 100)?;
    let warmed = ledger.stats().spare_ticket_capacity;
    if warmed < capacity_for(100) {
        return Err(format!("the warm frame was not kept (capacity {warmed})"));
    }
    let second_refused = match ledger.set_spare_limit(4) {
        Ok(()) => false,
        Err(SpareLimitFixed) => true,
        Err(error) => return Err(format!("the second setting failed otherwise: {error:?}")),
    };
    spare_unknown_frame(&ledger, 300, 2)?;
    let small_frame_capacity = ledger.stats().spare_ticket_capacity;
    spare_unknown_frame(&ledger, 400, 10)?;
    let ten_key_capacity = ledger.stats().spare_ticket_capacity;
    spare_shutdown(&ledger);
    Ok(SpareLimitSeen {
        second_refused,
        small_frame_capacity,
        ten_key_capacity,
    })
}

/// The refusals that need no control: a setting after a frame, and with a ticket building, are refused and the limit in force
/// (0 here) is unchanged.
fn spare_limit_refusals(context: &Arc<MetalContext>) -> Result<(), CaseError> {
    use ck_gpu_metal::LedgerError::SpareLimitFixed;
    let used = spare_ledger(context, false, None)?;
    spare_unknown_frame(&used, 100, 0)?;
    if used.set_spare_limit(4) != Err(SpareLimitFixed) {
        return assertion("a setting after a frame was not refused".to_owned());
    }
    spare_unknown_frame(&used, 100, 2)?;
    if used.stats().spare_ticket_capacity != 0 {
        return assertion("a refused setting changed the limit in force".to_owned());
    }
    let building = spare_ledger(context, false, None)?;
    let ticket = building
        .begin_frame()
        .map_err(|error| format!("begin: {error:?}"))?;
    if building.set_spare_limit(4) != Err(SpareLimitFixed) {
        return assertion("a setting with a ticket building was not refused".to_owned());
    }
    ticket
        .discarded()
        .map_err(|error| format!("discarded: {error:?}"))?;
    spare_shutdown(&used);
    spare_shutdown(&building);
    Ok(())
}

fn verify_spare_limit_is_fixed_at_setup(context: &Arc<MetalContext>) -> bool {
    // Normal: the second setting is refused, so the limit in force stays 1,000: a 2-key frame keeps the warmed allocation and a
    // 10-key frame (over 4) is kept too.
    let normal = match spare_limit_observe(context, None) {
        Ok(seen) => seen,
        Err(error) => {
            eprintln!("spare limit: setup failed: {error}");
            return false;
        }
    };
    println!("spare limit: observed {normal:?}");
    if !normal.second_refused
        || normal.small_frame_capacity < capacity_for(100)
        || normal.ten_key_capacity < 10
    {
        eprintln!("spare limit: the limit was not fixed at setup: {normal:?}");
        return false;
    }
    if let Err(error) = spare_limit_refusals(context) {
        eprintln!("spare limit: {error}");
        return false;
    }
    // Control: a setter that always applies lowers the limit to 4. Then the 2-key frame (within 4) keeps the warmed allocation, whose
    // capacity is above the standard map's for 4 entries (the r4 witness), and the 10-key frame (over 4) drops the map. A setup error
    // is not that outcome.
    let faulty = match spare_limit_observe(context, Some(ck_gpu_metal::SpareFault::AlwaysApply)) {
        Ok(seen) => seen,
        Err(error) => {
            eprintln!("spare limit: the control failed in setup, not as required: {error}");
            return false;
        }
    };
    println!("spare limit: AlwaysApply control observed {faulty:?}");
    let witness = !faulty.second_refused
        && faulty.small_frame_capacity > capacity_for(4)
        && faulty.ten_key_capacity == 0;
    if !witness {
        eprintln!(
            "spare limit: the control did not show the retained-capacity witness (want not refused, small-frame capacity over {}, ten-key capacity 0): {faulty:?}",
            capacity_for(4)
        );
        return false;
    }
    println!(
        "spare limit: fixed at setup, every later call refused, and the control shows its witness (capacity {} kept under a limit of 4, over {})",
        faulty.small_frame_capacity,
        capacity_for(4)
    );
    true
}

fn wait_ledger_prepared(
    context: &Arc<MetalContext>,
    ledger: &TextureLedger<u32>,
    key: u32,
    revision: u64,
    events: &mut Vec<LedgerEvent<u32>>,
) -> bool {
    const TIMEOUT: Duration = Duration::from_secs(5);
    const FOLLOWUP: Duration = Duration::from_secs(10);
    const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);
    const MAX_SNAPSHOTS: usize = 48;

    let wait_started = Instant::now();
    let until = wait_started + TIMEOUT;
    let mut next_snapshot = wait_started;
    let mut pump_count = 0_u64;
    let mut timeline = Vec::with_capacity(MAX_SNAPSHOTS);
    loop {
        ledger.pump(events);
        pump_count += 1;
        if !ledger_charges_conserved(&ledger.stats()) {
            eprintln!(
                "ledger charge conservation failed during upload completion: {:?}",
                ledger.stats()
            );
            return false;
        }
        if events.iter().any(|event| matches!(event, LedgerEvent::Prepared { key: found, revision: found_revision } if *found == key && *found_revision == revision)) {
            return true;
        }
        if Instant::now() >= until {
            let timeout_stats = ledger.stats();
            let timeout_unpolled = ledger.completion_arrived_unpolled();
            let timeout_events = events.clone();
            let timeout_live_objects = context.native_live_objects();
            eprintln!(
                "wait_ledger_prepared timeout: key={key}, revision={revision}, elapsed={:?}, pumps={pump_count}, completion_arrived_unpolled={timeout_unpolled}, stats={timeout_stats:?}, native_live_objects={timeout_live_objects:?}, events={timeout_events:?}, timeline={timeline:#?}",
                wait_started.elapsed()
            );

            // The original five-second wait remains a failure. This bounded follow-up only
            // records whether the late completion becomes observable; it cannot turn false into true.
            let followup_until = Instant::now() + FOLLOWUP;
            let mut next_followup_snapshot = Instant::now();
            while Instant::now() < followup_until {
                ledger.pump(events);
                pump_count += 1;
                let elapsed = wait_started.elapsed();
                if Instant::now() >= next_followup_snapshot {
                    let entry = format!(
                        "follow-up elapsed={elapsed:?} pumps={pump_count} completion_arrived_unpolled={} stats={:?} events={events:?}",
                        ledger.completion_arrived_unpolled(),
                        ledger.stats()
                    );
                    if timeline.len() == MAX_SNAPSHOTS {
                        timeline.remove(0);
                    }
                    timeline.push(entry);
                    next_followup_snapshot = Instant::now() + SNAPSHOT_INTERVAL;
                }
                if events.iter().any(|event| matches!(event, LedgerEvent::Prepared { key: found, revision: found_revision } if *found == key && *found_revision == revision)) {
                    break;
                }
                thread::yield_now();
            }
            eprintln!(
                "wait_ledger_prepared report-only follow-up: key={key}, revision={revision}, elapsed={:?}, pumps={pump_count}, completion_arrived_unpolled={}, stats={:?}, native_live_objects={:?}, events={events:?}, timeline={timeline:#?}; original wait remains failed",
                wait_started.elapsed(),
                ledger.completion_arrived_unpolled(),
                ledger.stats(),
                context.native_live_objects()
            );
            return false;
        }
        let now = Instant::now();
        if now >= next_snapshot {
            let entry = format!(
                "elapsed={:?} pumps={pump_count} completion_arrived_unpolled={} stats={:?} events={events:?}",
                wait_started.elapsed(),
                ledger.completion_arrived_unpolled(),
                ledger.stats()
            );
            if timeline.len() == MAX_SNAPSHOTS {
                timeline.remove(0);
            }
            timeline.push(entry);
            next_snapshot = now + SNAPSHOT_INTERVAL;
        }
        thread::yield_now();
    }
}

fn ledger_charges_conserved(stats: &ck_gpu_metal::LedgerStats) -> bool {
    stats.baseline_charges + stats.extra_revisions
        == stats.in_flight
            + stats.prepared
            + stats.committed_waiting_for_boundary
            + stats.active
            + stats.retired
}

fn begin_ledger_reader(
    ledger: &TextureLedger<u32>,
    key: u32,
    delay: Duration,
    error: bool,
) -> Result<
    (
        ck_gpu_metal::FrameCommandBuffer,
        ck_gpu_metal::FrameTicket<'_, u32>,
    ),
    String,
> {
    let ticket = ledger
        .begin_frame()
        .map_err(|error| format!("begin: {error:?}"))?;
    let texture = ticket
        .resolve(key)
        .ok_or_else(|| "no active revision".to_owned())?;
    let command = texture
        .test_reader(delay, error)
        .map_err(|error| error.to_string())?;
    drop(texture);
    Ok((command, ticket))
}

fn measure_throughput(
    port: &ck_gpu_metal::UploadPort<u32>,
    owner: &mut ck_gpu_metal::TextureOwner<u32>,
) -> Result<(f64, Vec<u8>), String> {
    let mut items = std::collections::VecDeque::with_capacity(2048);
    let mut shared_images = Vec::with_capacity(2048 * 32 * 32 * 4);
    for index in 0..2048_usize {
        let id = u32::try_from(index)
            .map_err(|error| error.to_string())?
            .to_le_bytes();
        let mut bytes = vec![0_u8; 32 * 32 * 4];
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.copy_from_slice(&id);
        }
        shared_images.extend_from_slice(&bytes);
        items.push_back(UploadItem {
            tag: u32::try_from(index).map_err(|error| error.to_string())?,
            width: 32,
            height: 32,
            pixels: Arc::new(Pixels { bytes, drops: None }),
        });
    }
    let started = Instant::now();
    let deadline = started + VERIFY_DEADLINE;
    let mut first_offer = None;
    let mut ready = 0_usize;
    let mut results = Vec::new();
    while ready < 2048 {
        while let Some(item) = items.pop_front() {
            match port.offer(item) {
                Ok(()) => {
                    first_offer.get_or_insert_with(Instant::now);
                }
                Err(ck_gpu_metal::Refused(item)) => {
                    items.push_front(item);
                    break;
                }
            }
        }
        owner.submit();
        owner.poll(&mut results);
        for result in results.drain(..) {
            match result {
                Completed::Ready(texture) => {
                    ready += 1;
                    drop(texture);
                }
                Completed::Failed { tag, reason } => {
                    return Err(format!("image {tag} failed: {reason}"));
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("only {ready}/2048 textures completed"));
        }
        thread::yield_now();
    }
    let elapsed = Instant::now().saturating_duration_since(first_offer.unwrap_or(started));
    if elapsed.is_zero() {
        return Err("throughput measurement interval was zero".to_owned());
    }
    Ok((2048.0 / elapsed.as_secs_f64(), shared_images))
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixture keeps dimensions, pixel checks and cleanup evidence in sequence"
)]
fn verify_pixel_matrix(context: &Arc<MetalContext>) -> bool {
    for (tag, width, height) in [
        (11_u32, 1_u32, 1_u32),
        (12, 3, 5),
        (13, 4096, 4096),
        (14, 8192, 2048),
    ] {
        let expected = pixel_bytes(tag, width, height);
        let (port, mut owner) = match TextureStore::start(
            Arc::clone(context),
            TextureLimits::default(),
            TextureWakes::default(),
        ) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("pixel matrix: start failed: {error}");
                return false;
            }
        };
        if port
            .offer(UploadItem {
                tag,
                width,
                height,
                pixels: Arc::new(Pixels {
                    bytes: expected.clone(),
                    drops: None,
                }),
            })
            .is_err()
        {
            eprintln!("pixel matrix: offer refused for {width}x{height}");
            return false;
        }
        let until = Instant::now() + VERIFY_DEADLINE;
        let mut completed = Vec::new();
        loop {
            owner.submit();
            owner.poll(&mut completed);
            if let Some(result) = completed.pop() {
                match result {
                    Completed::Ready(mut texture) => match texture.readback() {
                        Ok(actual) if actual == expected => {}
                        Ok(_) => {
                            eprintln!("pixel matrix: mismatch for {width}x{height}");
                            return false;
                        }
                        Err(error) => {
                            eprintln!("pixel matrix: readback failed: {error}");
                            return false;
                        }
                    },
                    Completed::Failed { reason, .. } => {
                        eprintln!("pixel matrix: upload failed: {reason}");
                        return false;
                    }
                }
                break;
            }
            if Instant::now() >= until {
                eprintln!("pixel matrix: timed out for {width}x{height}");
                return false;
            }
            thread::yield_now();
        }
        if !matches!(
            owner.shutdown(Instant::now() + Duration::from_secs(3)),
            ck_gpu_metal::StoreCleanup::Proven
        ) {
            eprintln!("pixel matrix: shutdown was unproven for {width}x{height}");
            return false;
        }
        let stats = owner.stats();
        if stats.texture_bytes_reserved != 0
            || stats.texture_bytes_measured != 0
            || stats.live_textures != 0
            || stats.staging_bytes != 0
        {
            eprintln!("pixel matrix: a reservation survived dropping the prepared texture");
            return false;
        }
    }

    // These three uploads fit in one ordinary envelope and exercise mixed row widths and offsets.
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            block_worker: Some(Arc::clone(&gate)),
            ..ck_gpu_metal::TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("mixed batch: start failed: {error}");
            return false;
        }
    };
    let expected: Vec<_> = [(21_u32, 1_u32, 1_u32), (22, 3, 5), (23, 32, 32)]
        .into_iter()
        .map(|(tag, width, height)| (tag, pixel_bytes(tag, width, height)))
        .collect();
    for (tag, bytes) in &expected {
        let (width, height) = match tag {
            21 => (1, 1),
            22 => (3, 5),
            _ => (32, 32),
        };
        if port
            .offer(UploadItem {
                tag: *tag,
                width,
                height,
                pixels: Arc::new(Pixels {
                    bytes: bytes.clone(),
                    drops: None,
                }),
            })
            .is_err()
        {
            eprintln!("mixed batch: offer refused");
            return false;
        }
    }
    let (mutex, changed) = &*gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut results = Vec::new();
    while results.len() < expected.len() {
        owner.submit();
        owner.poll(&mut results);
        if Instant::now() >= until {
            eprintln!("mixed batch: timed out");
            return false;
        }
        thread::yield_now();
    }
    let stats = owner.stats();
    let raw_bytes: usize = expected.iter().map(|(_, bytes)| bytes.len()).sum();
    if stats.batches_prepared != 1
        || stats.images_per_batch_max != 3
        || raw_bytes >= 16 * 1024
        || stats.staging_bytes_peak < 16 * 1024
    {
        eprintln!("mixed batch: sizes were not grouped into one ordered envelope");
        return false;
    }
    for result in results {
        match result {
            Completed::Ready(mut texture) => {
                let Some((_, bytes)) = expected.iter().find(|(tag, _)| *tag == texture.tag) else {
                    eprintln!("mixed batch: unknown result tag");
                    return false;
                };
                if !matches!(texture.readback(), Ok(actual) if actual == *bytes) {
                    eprintln!("mixed batch: readback mismatch");
                    return false;
                }
            }
            Completed::Failed { reason, .. } => {
                eprintln!("mixed batch: upload failed: {reason}");
                return false;
            }
        }
    }
    matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    )
}

fn verify_panicking_payload_wake(context: &Arc<MetalContext>) -> bool {
    let wakes = TextureWakes {
        ready_to_submit: Some(Arc::new(|| std::panic::panic_any(PanickingPayload))),
        completed: None,
        room: None,
    };
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        wakes,
        ck_gpu_metal::TextureTestHooks::default(),
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("panic payload wake: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 72_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![72; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.is_empty() {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("panic payload wake: progress failed after independent polling");
            return false;
        }
        thread::yield_now();
    }
    if owner.stats().wake_panics != 1 {
        eprintln!("panic payload wake: panic was not contained and counted");
        return false;
    }
    drop(completed);
    matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    )
}

fn pixel_bytes(seed: u32, width: u32, height: u32) -> Vec<u8> {
    let len = usize::try_from(width * height * 4).expect("valid fixture size");
    let seed = seed.to_le_bytes()[0];
    (0..len)
        .map(|index| seed.wrapping_add(index.to_le_bytes()[0]))
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "the fault and recovery matrix is easier to audit in execution order"
)]
fn verify_failure_paths(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;

    if !verify_window_and_batch_caps(context) {
        return false;
    }
    if !verify_staging_formation_raw_payload_negative_control(context)
        || !verify_staging_progress(context)
        || !verify_staging_stop_wake(context)
    {
        return false;
    }
    if !verify_oversized_image_is_alone(context) {
        return false;
    }
    if !verify_closed_envelope_proof(context) {
        return false;
    }

    for (name, hooks) in [
        (
            "nil staging",
            TextureTestHooks {
                nil_staging: true,
                ..TextureTestHooks::default()
            },
        ),
        (
            "nil command",
            TextureTestHooks {
                nil_command: true,
                ..TextureTestHooks::default()
            },
        ),
        (
            "nil encoder",
            TextureTestHooks {
                nil_encoder: true,
                ..TextureTestHooks::default()
            },
        ),
        (
            "nil texture",
            TextureTestHooks {
                fail_texture_allocation_at: Some(1),
                ..TextureTestHooks::default()
            },
        ),
        (
            "commit",
            TextureTestHooks {
                fail_commit: true,
                ..TextureTestHooks::default()
            },
        ),
        (
            "completion",
            TextureTestHooks {
                error_completion: true,
                ..TextureTestHooks::default()
            },
        ),
    ] {
        if !run_fatal_case(context, name, hooks, TextureWakes::default()) {
            return false;
        }
    }

    if !verify_failure_while_prior_batch_is_submitted(context) {
        return false;
    }
    if !verify_owner_commit_thread(context) {
        return false;
    }
    if !verify_estimate_refusal_with_native_failure(context) {
        return false;
    }

    if !verify_blocked_worker_recovery(context) || !verify_worker_panic(context) {
        return false;
    }

    if !run_fatal_case(
        context,
        "error completion with lost wake",
        TextureTestHooks {
            error_completion: true,
            panic_wake_completed: true,
            ..TextureTestHooks::default()
        },
        TextureWakes::default(),
    ) {
        return false;
    }
    if !verify_lost_completion_wake(context) {
        return false;
    }

    if !verify_delayed_shutdown_incomplete_proof(context)
        || !verify_incomplete_shutdown_proof_control(context)
        || !verify_shutdown_gate_release_after_proof_enters(context)
    {
        return false;
    }

    let native_baseline = context.native_live_objects();
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits {
            texture_bytes: 64 << 20,
            ..TextureLimits::default()
        },
        TextureWakes::default(),
        TextureTestHooks {
            force_measured_size: Some((64 << 20) + 1),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("measured budget refusal: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 63_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![63; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut result = Vec::new();
    while result.is_empty() {
        owner.submit();
        owner.poll(&mut result);
        if Instant::now() >= until {
            eprintln!("measured budget refusal: timed out");
            return false;
        }
        thread::yield_now();
    }
    if !matches!(result.pop(), Some(Completed::Failed { .. }))
        || context.native_live_objects().textures != native_baseline.textures
        || owner.take_failure().is_some()
        || owner.stats().texture_transient_peak <= 64 << 20
        // The native texture was created and then discarded by the measured-size check: it counts as created, and is
        // not alive.
        || owner.stats().textures_created != 1
        || owner.stats().live_textures != 0
        || !matches!(
            owner.shutdown(Instant::now() + Duration::from_secs(3)),
            ck_gpu_metal::StoreCleanup::Proven
        )
    {
        eprintln!(
            "measured budget refusal: not kept as a per-item failure or transient peak missing"
        );
        return false;
    }

    verify_lost_ready_wake(context)
        && verify_panicking_payload_wake(context)
        && verify_room_wake_panic(context)
        && verify_one_deadline(context)
}

fn verify_oversized_image_is_alone(context: &Arc<MetalContext>) -> bool {
    if !verify_oversized_early_release_control(context) {
        return false;
    }
    let worker_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            block_worker: Some(Arc::clone(&worker_gate)),
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("oversized batch: start failed: {error}");
            return false;
        }
    };
    for (tag, width, height) in [(101_u32, 1_u32, 1_u32), (102, 8192, 2048), (103, 1, 1)] {
        let length = usize::try_from(width * height * 4).expect("bounded image size");
        if port
            .offer(UploadItem {
                tag,
                width,
                height,
                pixels: Arc::new(Pixels {
                    bytes: vec![tag.to_le_bytes()[0]; length],
                    drops: None,
                }),
            })
            .is_err()
        {
            eprintln!("oversized batch: offer refused");
            return false;
        }
    }
    if let Err(reason) = oversized_batch_precondition(&owner.stats()) {
        eprintln!("oversized batch: precondition failed before worker release: {reason}");
        open_spill_worker(&worker_gate);
        return false;
    }
    let (mutex, changed) = &*worker_gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut results = Vec::new();
    while results.len() < 3 {
        owner.submit();
        owner.poll(&mut results);
        if Instant::now() >= until {
            eprintln!("oversized batch: timed out");
            return false;
        }
        thread::yield_now();
    }
    let tags: Vec<_> = results
        .iter()
        .map(|item| match item {
            Completed::Ready(texture) => Some(texture.tag),
            Completed::Failed { .. } => None,
        })
        .collect();
    if tags.iter().any(Option::is_none) {
        eprintln!("oversized batch: an admitted image failed");
        return false;
    }
    drop(results);
    let stats = owner.stats();
    if stats.batches_prepared != 3
        || stats.images_per_batch_max != 1
        || stats.staging_bytes_peak > TextureLimits::default().staging_bytes
    {
        eprintln!("oversized batch: the large image did not occupy a singleton bounded batch");
        return false;
    }
    matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    )
}

fn oversized_batch_precondition(stats: &ck_gpu_metal::TextureStats) -> Result<(), String> {
    if stats.prepared_images == 3 && stats.batches_prepared == 0 && stats.batches_submitted == 0 {
        Ok(())
    } else {
        Err(format!(
            "C precondition: expected 3 queued images and no prepared/submitted batch, got prepared_images={}, batches_prepared={}, batches_submitted={}",
            stats.prepared_images, stats.batches_prepared, stats.batches_submitted
        ))
    }
}

fn verify_oversized_early_release_control(context: &Arc<MetalContext>) -> bool {
    let completion_gate = Arc::new(AtomicI32::new(0));
    let worker_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            completion_gate: Some(Arc::clone(&completion_gate)),
            block_worker: Some(Arc::clone(&worker_gate)),
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("oversized early-release control: start failed: {error}");
            return false;
        }
    };
    let offered = port
        .offer(UploadItem {
            tag: 104_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![104; 4],
                drops: None,
            }),
        })
        .is_ok();
    open_spill_worker(&worker_gate);
    let validation = if offered {
        let deadline = Instant::now() + VERIFY_DEADLINE;
        while owner.stats().batches_submitted == 0 && Instant::now() < deadline {
            owner.submit();
            thread::yield_now();
        }
        if owner.stats().batches_submitted == 0 {
            Err("control upload was not submitted".to_owned())
        } else {
            match oversized_batch_precondition(&owner.stats()) {
                Err(reason) if reason.starts_with("C precondition:") => Ok(reason),
                Err(reason) => Err(format!("wrong precondition failure: {reason}")),
                Ok(()) => Err("early release unexpectedly satisfied the precondition".to_owned()),
            }
        }
    } else {
        Err("control upload was refused".to_owned())
    };
    completion_gate.store(1, Ordering::Release);
    let cleanup_until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.is_empty() && Instant::now() < cleanup_until {
        owner.poll(&mut completed);
        thread::yield_now();
    }
    let cleanup = owner.shutdown(Instant::now() + Duration::from_secs(3));
    match (validation, cleanup) {
        (Ok(reason), ck_gpu_metal::StoreCleanup::Proven) => {
            eprintln!("oversized early-release control: captured named value: {reason}");
            true
        }
        (validation, cleanup) => {
            eprintln!(
                "oversized early-release control failed: {validation:?}; cleanup={cleanup:?}"
            );
            false
        }
    }
}

fn verify_incomplete_shutdown_proof_keeps_charge(
    owner: &mut ck_gpu_metal::TextureOwner<u32>,
) -> Result<(), &'static str> {
    let before = owner.stats();
    let incomplete = owner.prove(Instant::now() + Duration::from_millis(20));
    if incomplete.is_complete() {
        return Err("outstanding completion was incorrectly proven");
    }
    owner.release(incomplete);
    let after_incomplete_release = owner.stats();
    if after_incomplete_release.live_textures != before.live_textures
        || after_incomplete_release.staging_bytes != before.staging_bytes
    {
        return Err("outstanding completion was incorrectly proven");
    }
    let retained = owner.stats();
    if retained.live_textures != before.live_textures
        || retained.staging_bytes != before.staging_bytes
        || retained.live_textures == 0
    {
        return Err("unproven state released a device-dependent charge");
    }
    Ok(())
}

fn verify_delayed_shutdown_incomplete_proof(context: &Arc<MetalContext>) -> bool {
    let completion_gate = Arc::new(AtomicI32::new(0));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits {
            max_prepared_images: 2,
            max_upload_images_per_batch: 1,
            ..TextureLimits::default()
        },
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            completion_gate: Some(Arc::clone(&completion_gate)),
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("delayed shutdown proof: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 66_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![66; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        eprintln!("delayed shutdown proof: upload was refused");
        completion_gate.store(1, Ordering::Release);
        let _ = owner.shutdown(Instant::now() + Duration::from_secs(3));
        return false;
    }
    let submit_until = Instant::now() + VERIFY_DEADLINE;
    while owner.stats().batches_submitted == 0 && Instant::now() < submit_until {
        owner.submit();
        thread::yield_now();
    }
    if owner.stats().batches_submitted != 1 {
        eprintln!("delayed shutdown proof: expected one submitted batch");
        completion_gate.store(1, Ordering::Release);
        let _ = owner.shutdown(Instant::now() + Duration::from_secs(3));
        return false;
    }
    if let Err(error) = verify_incomplete_shutdown_proof_keeps_charge(&mut owner) {
        eprintln!("delayed shutdown proof: {error}");
        completion_gate.store(1, Ordering::Release);
        let _ = owner.shutdown(Instant::now() + Duration::from_secs(3));
        return false;
    }
    completion_gate.store(1, Ordering::Release);
    if !matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    ) {
        eprintln!("delayed shutdown proof: later proof did not complete");
        return false;
    }
    eprintln!("delayed shutdown proof: closed-gate proof incomplete; later proof Proven");
    true
}

fn verify_incomplete_shutdown_proof_control(context: &Arc<MetalContext>) -> bool {
    let completion_gate = Arc::new(AtomicI32::new(0));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits {
            max_prepared_images: 2,
            max_upload_images_per_batch: 1,
            ..TextureLimits::default()
        },
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            completion_gate: Some(Arc::clone(&completion_gate)),
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("incomplete proof control: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 67_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![67; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        eprintln!("incomplete proof control: upload was refused");
        completion_gate.store(1, Ordering::Release);
        let _ = owner.shutdown(Instant::now() + Duration::from_secs(3));
        return false;
    }
    let submit_until = Instant::now() + VERIFY_DEADLINE;
    while owner.stats().batches_submitted == 0 && Instant::now() < submit_until {
        owner.submit();
        thread::yield_now();
    }
    if owner.stats().batches_submitted != 1 {
        eprintln!("incomplete proof control: expected one submitted batch");
        completion_gate.store(1, Ordering::Release);
        let _ = owner.shutdown(Instant::now() + Duration::from_secs(3));
        return false;
    }
    completion_gate.store(1, Ordering::Release);
    let completion_until = Instant::now() + VERIFY_DEADLINE;
    while !owner.completion_arrived_unpolled() && Instant::now() < completion_until {
        thread::yield_now();
    }
    if !owner.completion_arrived_unpolled() {
        eprintln!("incomplete proof control: completion was not held unpolled");
        let _ = owner.shutdown(Instant::now() + Duration::from_secs(3));
        return false;
    }
    let controlled_failure = verify_incomplete_shutdown_proof_keeps_charge(&mut owner);
    let cleanup = owner.shutdown(Instant::now() + Duration::from_secs(3));
    if controlled_failure != Err("outstanding completion was incorrectly proven")
        || !matches!(cleanup, ck_gpu_metal::StoreCleanup::Proven)
    {
        eprintln!(
            "incomplete proof control: expected the incomplete assertion to fail as a value; result={controlled_failure:?}, cleanup={cleanup:?}"
        );
        return false;
    }
    eprintln!("incomplete proof control: completed batch was observed unpolled before proof");
    true
}

fn verify_shutdown_gate_release_after_proof_enters(context: &Arc<MetalContext>) -> bool {
    let completion_gate = Arc::new(AtomicI32::new(0));
    let (port, owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits {
            max_prepared_images: 2,
            max_upload_images_per_batch: 1,
            ..TextureLimits::default()
        },
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            completion_gate: Some(Arc::clone(&completion_gate)),
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("delayed shutdown control: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 64_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![64; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let (result_sender, result_receiver) = std::sync::mpsc::channel();
    let mut owner = owner;
    let shutdown_thread = thread::spawn(move || {
        let submit_deadline = Instant::now() + VERIFY_DEADLINE;
        while !owner.submit() && Instant::now() < submit_deadline {
            thread::yield_now();
        }
        if owner.stats().batches_submitted != 1 {
            let _ = result_sender.send(Err("held upload did not submit".to_owned()));
            return;
        }
        let result = owner.shutdown(Instant::now() + Duration::from_secs(3));
        let _ = result_sender.send(Ok(result));
    });
    // The offer probe is retried only when room is available. Once shutdown enters prove(), it sets
    // the store's stopping state before its bounded worker/completion proof; a refused offer with
    // room available is the observable phase boundary that orders opening the held completion.
    let port = port.clone();
    let mut offered_probe = false;
    let until = Instant::now() + VERIFY_DEADLINE;
    let entered = loop {
        if port.room() > 0 {
            let probe = UploadItem {
                tag: 65_u32,
                width: 1,
                height: 1,
                pixels: Arc::new(Pixels {
                    bytes: vec![65; 4],
                    drops: None,
                }),
            };
            match port.offer(probe) {
                Ok(()) => offered_probe = true,
                Err(_) if port.room() > 0 => break true,
                Err(_) => {}
            }
        }
        if Instant::now() >= until {
            break false;
        }
        thread::yield_now();
    };
    completion_gate.store(1, Ordering::Release);
    let result = result_receiver.recv_timeout(Duration::from_secs(4));
    let joined = shutdown_thread.join().is_ok();
    let proven = result
        .as_ref()
        .is_ok_and(|cleanup| matches!(cleanup, Ok(ck_gpu_metal::StoreCleanup::Proven)));
    if !entered || !joined || !proven {
        eprintln!(
            "delayed shutdown control failed: proof_entered={entered}, queued_probe={offered_probe}, result={result:?}, joined={joined}"
        );
        return false;
    }
    eprintln!(
        "delayed shutdown control: completion gate opened after proof entered; cleanup Proven"
    );
    true
}

fn verify_closed_envelope_proof(context: &Arc<MetalContext>) -> bool {
    let native_baseline = context.native_live_objects();
    let ready = Arc::new(AtomicUsize::new(0));
    let ready_wake = Arc::clone(&ready);
    let wakes = TextureWakes {
        ready_to_submit: Some(Arc::new(move || {
            ready_wake.fetch_add(1, Ordering::SeqCst);
        })),
        completed: None,
        room: None,
    };
    let (port, mut owner) =
        match TextureStore::start(Arc::clone(context), TextureLimits::default(), wakes) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("closed envelope proof: start failed: {error}");
                return false;
            }
        };
    if port
        .offer(UploadItem {
            tag: 85_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![85; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    while Instant::now() < until {
        let stats = owner.stats();
        let live = context.native_live_objects();
        if ready.load(Ordering::SeqCst) != 0
            && stats.live_textures == 1
            && stats.staging_bytes != 0
            && live.textures == native_baseline.textures + 1
            && live.batches == native_baseline.batches + 1
        {
            break;
        }
        thread::yield_now();
    }
    let before = owner.stats();
    let live_before = context.native_live_objects();
    if ready.load(Ordering::SeqCst) == 0
        || before.live_textures != 1
        || before.staging_bytes == 0
        || live_before.textures != native_baseline.textures + 1
        || live_before.batches != native_baseline.batches + 1
    {
        eprintln!(
            "closed envelope proof: worker did not retain a closed lease: ready={}, stats_textures={}, staging={}, native_textures={}/{}, native_batches={}/{}, baseline={native_baseline:?}, live={live_before:?}",
            ready.load(Ordering::SeqCst),
            before.live_textures,
            before.staging_bytes,
            live_before.textures,
            native_baseline.textures + 1,
            live_before.batches,
            native_baseline.batches + 1,
        );
        return false;
    }
    let proof = owner.prove(Instant::now() + Duration::from_secs(3));
    if owner.stats().live_textures != before.live_textures
        || owner.stats().staging_bytes != before.staging_bytes
        || context.native_live_objects() != live_before
    {
        eprintln!("closed envelope proof: proof released resources before the release phase");
        return false;
    }
    owner.release(proof);
    let after = owner.stats();
    let native_after = context.native_live_objects();
    if after.live_textures != 0
        || after.texture_bytes_reserved != 0
        || after.texture_bytes_measured != 0
        || after.staging_bytes != 0
        // Release-order sequences are monotonic diagnostics; compare all live counts and keep order assertions separate.
        || !same_native_live_counts(native_after, native_baseline)
    {
        eprintln!("closed envelope proof: release left a resource charge behind");
        return false;
    }
    true
}

/// Compares all six native live counts, excluding the two monotonic release-order diagnostics.
fn same_native_live_counts(
    left: ck_gpu_metal::NativeLiveObjects,
    right: ck_gpu_metal::NativeLiveObjects,
) -> bool {
    (
        left.contexts,
        left.textures,
        left.batches,
        left.frame_readers,
        left.residency_sets,
        left.residency_members,
    ) == (
        right.contexts,
        right.textures,
        right.batches,
        right.frame_readers,
        right.residency_sets,
        right.residency_members,
    )
}

fn verify_failure_while_prior_batch_is_submitted(context: &Arc<MetalContext>) -> bool {
    let limits = TextureLimits {
        max_prepared_images: 2,
        max_upload_images_per_batch: 1,
        max_upload_bytes_per_batch: 1 << 20,
        ..TextureLimits::default()
    };
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        limits,
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            fail_texture_allocation_at: Some(2),
            completion_delay: Duration::from_millis(300),
            ..ck_gpu_metal::TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("N+1 native failure: start failed: {error}");
            return false;
        }
    };
    for tag in [91_u32, 92] {
        if port
            .offer(UploadItem {
                tag,
                width: 1,
                height: 1,
                pixels: Arc::new(Pixels {
                    bytes: vec![tag.to_le_bytes()[0]; 4],
                    drops: None,
                }),
            })
            .is_err()
        {
            return false;
        }
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    let mut failure_seen = false;
    while completed.len() < 2 || !failure_seen {
        owner.submit();
        owner.poll(&mut completed);
        if owner.take_failure().is_some() {
            if failure_seen {
                eprintln!("N+1 native failure: failure was reported twice");
                return false;
            }
            failure_seen = true;
        }
        if Instant::now() >= until {
            eprintln!("N+1 native failure: timed out");
            return false;
        }
        thread::yield_now();
    }
    let first_ready = completed
        .iter()
        .any(|item| matches!(item, Completed::Ready(texture) if texture.tag == 91));
    let second_failed = completed
        .iter()
        .any(|item| matches!(item, Completed::Failed { tag: 92, .. }));
    if !first_ready || !second_failed {
        eprintln!("N+1 native failure: earlier submitted work did not keep its own result");
        return false;
    }
    // The first allocation succeeded, the second failed natively: only the first is a creation.
    if owner.stats().textures_created != 1 {
        eprintln!(
            "N+1 native failure: {} creation(s) counted, not exactly the one that succeeded",
            owner.stats().textures_created
        );
        return false;
    }
    drop(completed);
    if !matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    ) {
        eprintln!("N+1 native failure: unsubmitted failed lease was not reclaimed after proof");
        return false;
    }
    true
}

fn verify_owner_commit_thread(context: &Arc<MetalContext>) -> bool {
    let observed = Arc::new(Mutex::new(ck_gpu_metal::TestThreadObservation::default()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            thread_observer: Some(Arc::clone(&observed)),
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("owner commit thread: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 206_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![206; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.is_empty() {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("owner commit thread: upload timed out");
            return false;
        }
        thread::yield_now();
    }
    let observation = observed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let valid = observation.worker.is_some()
        && observation.commits == [thread::current().id()]
        && observation.commits.first() != observation.worker.as_ref();
    drop(observation);
    drop(completed);
    if !valid
        || !matches!(
            owner.shutdown(Instant::now() + Duration::from_secs(3)),
            ck_gpu_metal::StoreCleanup::Proven
        )
    {
        eprintln!("owner commit thread: native commit was not confined to the caller thread");
        return false;
    }
    true
}

fn verify_window_and_batch_caps(context: &Arc<MetalContext>) -> bool {
    let limits = TextureLimits {
        max_prepared_images: 2,
        max_upload_images_per_batch: 1,
        max_upload_bytes_per_batch: 1 << 20,
        ..TextureLimits::default()
    };
    let (port, mut owner) =
        match TextureStore::start(Arc::clone(context), limits, TextureWakes::default()) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("window caps: start failed: {error}");
                return false;
            }
        };
    for tag in [1_u32, 2] {
        if port
            .offer(UploadItem {
                tag,
                width: 1,
                height: 1,
                pixels: Arc::new(Pixels {
                    bytes: vec![tag.to_le_bytes()[0]; 4],
                    drops: None,
                }),
            })
            .is_err()
        {
            eprintln!("window caps: admitted offer was refused");
            return false;
        }
    }
    let refused = match port.offer(UploadItem {
        tag: 3,
        width: 1,
        height: 1,
        pixels: Arc::new(Pixels {
            bytes: vec![3; 4],
            drops: None,
        }),
    }) {
        Ok(()) => {
            eprintln!("window caps: accepted beyond the prepared window");
            return false;
        }
        Err(ck_gpu_metal::Refused(item)) => item.tag == 3,
    };
    if !refused || port.room() != 0 {
        eprintln!("window caps: refusal did not return its item or report a full window");
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut results = Vec::new();
    while results.len() < 2 {
        owner.submit();
        owner.poll(&mut results);
        if Instant::now() >= until {
            eprintln!("window caps: timed out");
            return false;
        }
        thread::yield_now();
    }
    if results
        .iter()
        .any(|item| !matches!(item, Completed::Ready(_)))
    {
        eprintln!("window caps: an admitted upload failed");
        return false;
    }
    drop(results);
    let stats = owner.stats();
    if stats.images_per_batch_max > 1
        || stats.prepared_images_peak != 2
        || stats.staging_bytes_peak > limits.staging_bytes
        || port.room() != 2
    {
        eprintln!("window caps: observed batch or reservation counters exceeded their caps");
        return false;
    }
    if stats.texture_bytes_reserved != 0
        || stats.texture_bytes_measured != 0
        || stats.live_textures != 0
        || stats.staging_bytes != 0
    {
        eprintln!("window caps: resource charges did not return to zero");
        return false;
    }
    matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "the edge-limit, wait, and recovery sequence is one staging scenario"
)]
fn verify_staging_progress(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let limits = TextureLimits {
        max_prepared_images: 10,
        max_upload_images_per_batch: 4,
        max_upload_bytes_per_batch: 64 << 20,
        staging_bytes: 128 << 20,
        ..TextureLimits::default()
    };
    let reservation = Arc::new(std::sync::atomic::AtomicU64::new(64 << 20));
    let worker_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        limits,
        TextureWakes::default(),
        TextureTestHooks {
            staging_reservation: Some(Arc::clone(&reservation)),
            block_worker: Some(Arc::clone(&worker_gate)),
            completion_delay: Duration::from_millis(350),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("staging progress: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 201_u32,
            width: 4096,
            height: 4096,
            pixels: Arc::new(Pixels {
                bytes: vec![0x5a; 64 << 20],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    for (tag, width, height) in [
        (202_u32, 1_u32, 1_u32),
        (203, 3, 5),
        (204, 5, 2),
        (205, 2, 7),
        (206, 7, 3),
        (207, 4, 4),
        (208, 1, 9),
    ] {
        if port
            .offer(UploadItem {
                tag,
                width,
                height,
                pixels: Arc::new(Pixels {
                    bytes: pixel_bytes(tag, width, height),
                    drops: None,
                }),
            })
            .is_err()
        {
            eprintln!("staging progress: small item refused");
            return false;
        }
    }
    let (mutex, changed) = &*worker_gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    let until = Instant::now() + VERIFY_DEADLINE;
    while !owner.submit() {
        if Instant::now() >= until {
            eprintln!("staging progress: first lease did not submit");
            return false;
        }
        thread::yield_now();
    }
    while owner.stats().staging_waits == 0 {
        if let Some(failure) = owner.take_failure() {
            eprintln!(
                "staging progress: store failed before room wait: {failure}; stats={:?}; native={:?}",
                owner.stats(),
                context.native_live_objects(),
            );
            return false;
        }
        if Instant::now() >= until {
            eprintln!(
                "staging progress: second lease never waited for room; stats={:?}; native={:?}",
                owner.stats(),
                context.native_live_objects(),
            );
            return false;
        }
        thread::yield_now();
    }
    let mut completed = Vec::new();
    while completed.is_empty() {
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("staging progress: delayed completion timed out");
            return false;
        }
        thread::yield_now();
    }
    // Keep the synthetic reservation until the second batch is actually prepared. If
    // completion failed to return its staging charge or notify the worker, this wait would
    // time out: clearing the reservation first would be a false negative control.
    while owner.stats().batches_prepared < 2 {
        if let Some(failure) = owner.take_failure() {
            eprintln!(
                "staging progress: store failed while waiting for the second batch: {failure}"
            );
            return false;
        }
        if Instant::now() >= until {
            eprintln!(
                "staging progress: completion did not wake the second batch with reservation held"
            );
            return false;
        }
        thread::yield_now();
    }
    reservation.store(0, Ordering::Release);
    owner.poll(&mut completed);
    while completed.len() < 8 {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("staging progress: bounded waiter did not resume");
            return false;
        }
        thread::yield_now();
    }
    let stats = owner.stats();
    if completed
        .iter()
        .any(|item| !matches!(item, Completed::Ready(_)))
        || stats.batches_prepared != 3
        || stats.images_per_batch_max != 4
        || stats.staging_waits == 0
        || stats.staging_bytes_peak > limits.staging_bytes
    {
        eprintln!(
            "staging progress: assertions failed: completed={}, ready={}, batches={}, max_batch={}, waits={}, staged_peak={}, reservation={}",
            completed.len(),
            completed
                .iter()
                .filter(|item| matches!(item, Completed::Ready(_)))
                .count(),
            stats.batches_prepared,
            stats.images_per_batch_max,
            stats.staging_waits,
            stats.staging_bytes_peak,
            reservation.load(Ordering::Acquire),
        );
        return false;
    }
    drop(completed);
    matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    )
}

fn verify_staging_formation_raw_payload_negative_control(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;

    let limits = TextureLimits {
        max_prepared_images: 4,
        max_upload_images_per_batch: 2,
        max_upload_bytes_per_batch: 4 << 20,
        staging_bytes: 128 << 20,
        ..TextureLimits::default()
    };
    // Raw payload totals exactly 4 MiB, so raw-byte formation would admit both images.
    // The first image rounds to 4 MiB of staging; the next image and the batch's 16 KiB
    // granularity exceed the cap and must force two batches.
    let raw_payload = u64::from(1023_u32) * u64::from(1025_u32) * 4 + 4;
    if raw_payload != 4 << 20 {
        eprintln!("staging formation control: fixture raw payload is {raw_payload}, not 4 MiB");
        return false;
    }
    let worker_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        limits,
        TextureWakes::default(),
        TextureTestHooks {
            block_worker: Some(Arc::clone(&worker_gate)),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("staging formation control: start failed: {error}");
            return false;
        }
    };
    for (tag, width, height) in [(301_u32, 1023_u32, 1025_u32), (302, 1, 1)] {
        if port
            .offer(UploadItem {
                tag,
                width,
                height,
                pixels: Arc::new(Pixels {
                    bytes: pixel_bytes(tag, width, height),
                    drops: None,
                }),
            })
            .is_err()
        {
            eprintln!("staging formation control: item {tag} was refused");
            return false;
        }
    }
    let (mutex, changed) = &*worker_gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();

    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.len() < 2 {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("staging formation control: timed out");
            return false;
        }
        thread::yield_now();
    }
    let stats = owner.stats();
    let tags: Vec<_> = completed
        .iter()
        .map(|item| match item {
            Completed::Ready(texture) => texture.tag,
            Completed::Failed { tag, .. } => *tag,
        })
        .collect();
    if tags != [301, 302]
        || completed
            .iter()
            .any(|item| !matches!(item, Completed::Ready(_)))
        || stats.batches_prepared != 2
        || stats.images_per_batch_max != 1
    {
        eprintln!(
            "staging formation control: raw-byte negative control failed: tags={tags:?}, batches={}, max_batch={}",
            stats.batches_prepared, stats.images_per_batch_max,
        );
        return false;
    }
    drop(completed);
    matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    )
}

fn verify_staging_stop_wake(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let reservation = Arc::new(std::sync::atomic::AtomicU64::new(128 << 20));
    let room_port = Arc::new(Mutex::new(None::<ck_gpu_metal::UploadPort<u32>>));
    let room_hook_calls = Arc::new(AtomicUsize::new(0));
    let room_observed = Arc::new(AtomicUsize::new(usize::MAX));
    let hook_port = Arc::clone(&room_port);
    let hook_calls = Arc::clone(&room_hook_calls);
    let hook_room = Arc::clone(&room_observed);
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes {
            room: Some(Arc::new(move || {
                let port = hook_port
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .cloned();
                if let Some(port) = port {
                    hook_room.store(port.room(), Ordering::SeqCst);
                    hook_calls.fetch_add(1, Ordering::SeqCst);
                }
            })),
            ..TextureWakes::default()
        },
        TextureTestHooks {
            staging_reservation: Some(reservation),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("staging stop: start failed: {error}");
            return false;
        }
    };
    *room_port
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(port.clone());
    if port
        .offer(UploadItem {
            tag: 205_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![205; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + Duration::from_secs(3);
    while owner.stats().staging_waits == 0 {
        if Instant::now() >= until {
            eprintln!("staging stop: worker did not enter staging wait");
            return false;
        }
        thread::yield_now();
    }
    let room_before_stop = port.room();
    let proof = owner.prove(Instant::now() + Duration::from_secs(1));
    if !proof.is_complete() {
        eprintln!(
            "staging stop: worker did not finish its stop notification before the proof deadline"
        );
        // A missing unlock leaves the worker blocked inside the room callback. Keep the owner's
        // graph alive instead of blocking in Drop while the regression reports the bounded failure.
        std::mem::forget(owner);
        return false;
    }
    room_port
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let mut completed = Vec::new();
    owner.poll(&mut completed);
    let stats = owner.stats();
    let shutdown = owner.shutdown(Instant::now() + Duration::from_secs(3));
    if !matches!(shutdown, ck_gpu_metal::StoreCleanup::Proven)
        || completed.len() != 1
        || !matches!(completed.first(), Some(Completed::Failed { .. }))
        || stats.staging_bytes != 0
        || stats.staging_waits == 0
        || room_hook_calls.load(Ordering::SeqCst) != 1
        || room_observed.load(Ordering::SeqCst) != room_before_stop + 1
    {
        eprintln!(
            "staging stop: stop notification or shutdown proof failed (room hooks={}, observed={}, expected={})",
            room_hook_calls.load(Ordering::SeqCst),
            room_observed.load(Ordering::SeqCst),
            room_before_stop + 1,
        );
        return false;
    }
    true
}

fn verify_estimate_refusal_with_native_failure(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits {
            max_prepared_images: 6,
            max_upload_images_per_batch: 3,
            texture_bytes: 64 << 20,
            ..TextureLimits::default()
        },
        TextureWakes::default(),
        TextureTestHooks {
            block_worker: Some(Arc::clone(&gate)),
            force_estimate_at: Some((2, 64 << 20)),
            fail_texture_allocation_at: Some(2),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("estimate refusal: start failed: {error}");
            return false;
        }
    };
    for tag in [211_u32, 212, 213] {
        if port
            .offer(UploadItem {
                tag,
                width: 1,
                height: 1,
                pixels: Arc::new(Pixels {
                    bytes: vec![tag.to_le_bytes()[0]; 4],
                    drops: None,
                }),
            })
            .is_err()
        {
            return false;
        }
    }
    let (mutex, changed) = &*gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.len() < 3 {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("estimate refusal: timed out");
            return false;
        }
        thread::yield_now();
    }
    let budget_reason = completed.iter().find_map(|item| match item {
        Completed::Failed { tag: 212, reason } => Some(reason),
        _ => None,
    });
    let native_reason = completed.iter().find_map(|item| match item {
        Completed::Failed { tag: 213, reason } => Some(reason),
        _ => None,
    });
    let ok = budget_reason.is_some_and(|reason| reason.contains("estimate"))
        && native_reason
            .is_some_and(|reason| reason.contains("Private Metal texture allocation failed"))
        && owner.take_failure().is_some()
        && matches!(
            owner.shutdown(Instant::now() + Duration::from_secs(3)),
            ck_gpu_metal::StoreCleanup::Proven
        );
    if !ok {
        eprintln!("estimate refusal: budget item lost its per-item reason during native failure");
    }
    ok
}

fn verify_blocked_worker_recovery(context: &Arc<MetalContext>) -> bool {
    verify_blocked_idle_worker_recovery(context) && verify_blocked_native_worker_recovery(context)
}

fn verify_blocked_idle_worker_recovery(context: &Arc<MetalContext>) -> bool {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (mut owner, shared_gate) = match TextureStore::start_with_test_hooks::<u32>(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            block_worker: Some(Arc::clone(&gate)),
            ..ck_gpu_metal::TextureTestHooks::default()
        },
    ) {
        Ok((_port, owner)) => (owner, gate),
        Err(error) => {
            eprintln!("blocked worker: start failed: {error}");
            return false;
        }
    };
    let started = Instant::now();
    let first = owner.shutdown(started + Duration::from_millis(20));
    let elapsed = started.elapsed();
    report_class_t_margin(
        "blocked worker shutdown",
        elapsed,
        Duration::from_millis(150),
    );
    if !matches!(first, ck_gpu_metal::StoreCleanup::Unproven { .. })
        || elapsed > Duration::from_millis(150)
    {
        eprintln!("blocked worker: Unproven did not return within deadline plus 130 ms");
        return false;
    }
    let (mutex, changed) = &*shared_gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    if !matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    ) {
        eprintln!("blocked worker: later worker proof did not complete");
        return false;
    }
    true
}

fn verify_blocked_native_worker_recovery(context: &Arc<MetalContext>) -> bool {
    let baseline = context.native_live_objects();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            block_after_texture: Some(Arc::clone(&gate)),
            ..ck_gpu_metal::TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("blocked native worker: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 82_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![82; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + Duration::from_secs(3);
    while context.native_live_objects().textures == baseline.textures && Instant::now() < until {
        thread::yield_now();
    }
    let retained = context.native_live_objects();
    if retained.textures != baseline.textures + 1
        || retained.batches != baseline.batches + 1
        || !matches!(
            owner.shutdown(Instant::now() + Duration::from_millis(20)),
            ck_gpu_metal::StoreCleanup::Unproven { .. }
        )
        || context.native_live_objects() != retained
    {
        eprintln!("blocked native worker: timeout did not retain the in-preparation graph");
        return false;
    }
    let (mutex, changed) = &*gate;
    *mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    changed.notify_all();
    if !matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    ) || !same_native_live_counts(context.native_live_objects(), baseline)
    {
        eprintln!("blocked native worker: later proof did not release retained graph");
        return false;
    }
    true
}

fn verify_worker_panic(context: &Arc<MetalContext>) -> bool {
    for (after_batch, after_texture, payload_panics) in [
        (true, false, false),
        (false, true, false),
        (false, true, true),
    ] {
        let baseline = context.native_live_objects();
        let (port, mut owner) = match TextureStore::start_with_test_hooks(
            Arc::clone(context),
            TextureLimits::default(),
            TextureWakes::default(),
            ck_gpu_metal::TextureTestHooks {
                panic_worker_after_batch: after_batch,
                panic_worker_after_texture: after_texture,
                panic_worker_payload_drop_panics: payload_panics,
                ..ck_gpu_metal::TextureTestHooks::default()
            },
        ) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("worker panic: start failed: {error}");
                return false;
            }
        };
        if port
            .offer(UploadItem {
                tag: 81_u32,
                width: 1,
                height: 1,
                pixels: Arc::new(Pixels {
                    bytes: vec![81; 4],
                    drops: None,
                }),
            })
            .is_err()
        {
            return false;
        }
        let until = Instant::now() + Duration::from_secs(3);
        while owner.take_failure().is_none() {
            if Instant::now() >= until {
                eprintln!("worker panic: fault hook did not run");
                return false;
            }
            thread::yield_now();
        }
        let retained = context.native_live_objects();
        let expected_textures = baseline.textures + u64::from(after_texture);
        if retained.textures != expected_textures || retained.batches != baseline.batches + 1 {
            eprintln!("worker panic: in-preparation native holders were not retained");
            return false;
        }
        match owner.shutdown(Instant::now() + Duration::from_secs(3)) {
            ck_gpu_metal::StoreCleanup::Unproven { reason }
                if reason.contains("test upload worker panic")
                    && context.native_live_objects() == retained => {}
            result => {
                eprintln!("worker panic: expected retained unproven state, got {result:?}");
                return false;
            }
        }
    }
    true
}

#[expect(
    clippy::too_many_lines,
    reason = "the lost-wake handshake and independent recovery form one scenario"
)]
fn verify_lost_ready_wake(context: &Arc<MetalContext>) -> bool {
    let drops = Arc::new(AtomicUsize::new(0));
    let wakes = TextureWakes::default();
    let (port, owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        wakes,
        ck_gpu_metal::TextureTestHooks {
            completion_delay: Duration::from_millis(200),
            panic_wake_ready: true,
            ..ck_gpu_metal::TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("wake panic: start failed: {error}");
            return false;
        }
    };
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker_entered = Arc::clone(&entered);
    let worker_drops = Arc::clone(&drops);
    let owner_thread = thread::spawn(move || {
        let mut owner = owner;
        let mut completed = Vec::new();
        owner.submit();
        owner.poll(&mut completed);
        let (lock, changed) = &*worker_entered;
        let mut idle = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *idle = true;
        changed.notify_one();
        let (idle, wait) = changed
            .wait_timeout(idle, Duration::from_millis(250))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(idle);
        if !wait.timed_out() {
            let _ = sender.send(Err(
                "the lost wake unexpectedly released the idle owner".to_owned()
            ));
            return;
        }
        let deadline = Instant::now() + VERIFY_DEADLINE;
        while completed.is_empty() {
            owner.submit();
            owner.poll(&mut completed);
            if Instant::now() >= deadline {
                let _ = sender.send(Err(
                    "independent polling did not recover the batch".to_owned()
                ));
                return;
            }
            thread::yield_now();
        }
        if owner.stats().wake_panics != 1 || worker_drops.load(Ordering::SeqCst) != 1 {
            let _ = sender.send(Err(
                "panic containment or pre-completion source drop failed".to_owned(),
            ));
            return;
        }
        drop(completed);
        let result = owner.shutdown(Instant::now() + Duration::from_secs(3));
        let _ = sender.send(if matches!(result, ck_gpu_metal::StoreCleanup::Proven) {
            Ok(())
        } else {
            Err("shutdown after independent polling was unproven".to_owned())
        });
    });
    let (lock, changed) = &*entered;
    let mut idle = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while !*idle {
        let (next, timeout) = changed
            .wait_timeout(idle, Duration::from_secs(3))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        idle = next;
        if timeout.timed_out() && !*idle {
            eprintln!("wake panic: owner did not reach its idle boundary");
            return false;
        }
    }
    drop(idle);
    if port
        .offer(UploadItem {
            tag: 71_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![7; 4],
                drops: Some(Arc::clone(&drops)),
            }),
        })
        .is_err()
    {
        return false;
    }
    let drop_until = Instant::now() + Duration::from_secs(3);
    while drops.load(Ordering::SeqCst) == 0 && Instant::now() < drop_until {
        thread::yield_now();
    }
    let source_dropped_while_owner_idle = drops.load(Ordering::SeqCst) == 1;
    let result = receiver.recv_timeout(Duration::from_secs(5));
    owner_thread.join().is_ok() && source_dropped_while_owner_idle && matches!(result, Ok(Ok(())))
}

fn verify_room_wake_panic(context: &Arc<MetalContext>) -> bool {
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            panic_wake_room: true,
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("room wake panic: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 221_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![221; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.is_empty() {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("room wake panic: upload did not complete");
            return false;
        }
        thread::yield_now();
    }
    let ok = owner.stats().wake_panics == 1
        && port.room() == TextureLimits::default().max_prepared_images
        && matches!(
            owner.shutdown(Instant::now() + Duration::from_secs(3)),
            ck_gpu_metal::StoreCleanup::Proven
        );
    if !ok {
        eprintln!("room wake panic: panic escaped, was uncounted, or altered room state");
    }
    ok
}

#[expect(
    clippy::too_many_lines,
    reason = "the gated lost-wake control and corrected bounded observation share one ordered test"
)]
fn verify_lost_completion_wake(context: &Arc<MetalContext>) -> bool {
    let completion_gate = Arc::new(AtomicI32::new(0));
    let (port, owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        TextureWakes::default(),
        ck_gpu_metal::TextureTestHooks {
            error_completion: true,
            completion_gate: Some(Arc::clone(&completion_gate)),
            panic_wake_completed: true,
            ..Default::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("lost completion wake: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 222_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![222; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        return false;
    }
    // 1 = old observation is waiting; 2 = the corrected bounded observation has started.
    let entered = Arc::new((Mutex::new(0_u8), Condvar::new()));
    let thread_entered = Arc::clone(&entered);
    let thread_gate = Arc::clone(&completion_gate);
    let owner_gate = Arc::clone(&thread_gate);
    let (sender, receiver) = std::sync::mpsc::channel();
    let owner_thread = thread::spawn(move || {
        let mut owner = owner;
        let until = Instant::now() + Duration::from_secs(3);
        while !owner.submit() {
            if Instant::now() >= until {
                let _ = sender.send(Err("batch did not submit".to_owned()));
                return;
            }
            thread::yield_now();
        }
        let mut completed = Vec::new();
        owner.poll(&mut completed);
        let (mutex, changed) = &*thread_entered;
        let mut idle = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *idle = 1;
        changed.notify_one();
        let (idle, wait) = changed
            .wait_timeout(idle, Duration::from_millis(250))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(idle);
        let control_saw_unpublished = wait.timed_out()
            && owner_gate.load(Ordering::Acquire) == 0
            && !owner.completion_arrived_unpolled();
        if !control_saw_unpublished {
            let _ = sender.send(Err(
                "completion gate did not hold the event through the old observation".to_owned(),
            ));
            return;
        }
        let completion_until = Instant::now() + Duration::from_secs(5);
        let mut announced_wait = false;
        loop {
            if owner.completion_arrived_unpolled() {
                break;
            }
            if !announced_wait {
                // Announce from inside the corrected bounded observation, after its first
                // unpublished read, so the control cannot open the gate before the wait begins.
                let mut phase = mutex
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *phase = 2;
                changed.notify_one();
                drop(phase);
                announced_wait = true;
            }
            if Instant::now() >= completion_until {
                break;
            }
            thread::yield_now();
        }
        if !owner.completion_arrived_unpolled() {
            let _ = sender.send(Err("bounded completion observation timed out".to_owned()));
            return;
        }
        owner.poll(&mut completed);
        let wake_until = Instant::now() + Duration::from_secs(1);
        while owner.stats().wake_panics != 1 && Instant::now() < wake_until {
            thread::yield_now();
        }
        let valid = completed
            .iter()
            .any(|item| matches!(item, Completed::Failed { tag: 222, .. }))
            && owner.take_failure().is_some()
            && owner.stats().wake_panics == 1;
        let cleanup = owner.shutdown(Instant::now() + Duration::from_secs(3));
        let _ = sender.send(
            if valid && matches!(cleanup, ck_gpu_metal::StoreCleanup::Proven) {
                Ok(())
            } else {
                Err("independent poll did not observe contained completion failure".to_owned())
            },
        );
    });
    let (mutex, changed) = &*entered;
    let mut idle = mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while *idle < 2 {
        let (next, timeout) = changed
            .wait_timeout(idle, Duration::from_secs(3))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        idle = next;
        if timeout.timed_out() && *idle < 2 {
            eprintln!("lost completion wake: corrected bounded observation never started");
            return false;
        }
    }
    drop(idle);
    thread_gate.store(1, Ordering::Release);
    let result = receiver.recv_timeout(Duration::from_secs(5));
    eprintln!("lost completion wake: owner result before join: {result:?}");
    owner_thread.join().is_ok() && matches!(result, Ok(Ok(())))
}

fn verify_one_deadline(context: &Arc<MetalContext>) -> bool {
    use ck_gpu_metal::TextureTestHooks;
    let baseline = context.native_live_objects();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits {
            max_prepared_images: 4,
            max_upload_images_per_batch: 1,
            ..TextureLimits::default()
        },
        TextureWakes::default(),
        TextureTestHooks {
            completion_delay: Duration::from_millis(600),
            block_after_texture: Some(Arc::clone(&gate)),
            block_after_texture_at: Some(2),
            ..TextureTestHooks::default()
        },
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("single deadline: start failed: {error}");
            return false;
        }
    };
    for tag in [231_u32, 232] {
        if port
            .offer(UploadItem {
                tag,
                width: 1,
                height: 1,
                pixels: Arc::new(Pixels {
                    bytes: vec![tag.to_le_bytes()[0]; 4],
                    drops: None,
                }),
            })
            .is_err()
        {
            return false;
        }
        if tag == 231 {
            let until = Instant::now() + Duration::from_secs(3);
            while !owner.submit() {
                if Instant::now() >= until {
                    eprintln!("single deadline: first upload did not submit");
                    return false;
                }
                thread::yield_now();
            }
        } else {
            let until = Instant::now() + Duration::from_secs(3);
            while context.native_live_objects().textures < baseline.textures + 2
                && Instant::now() < until
            {
                thread::yield_now();
            }
        }
    }
    let release_gate = Arc::clone(&gate);
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(350));
        let (mutex, changed) = &*release_gate;
        *mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    });
    let started = Instant::now();
    let result = owner.shutdown(started + Duration::from_millis(500));
    let elapsed = started.elapsed();
    report_class_t_margin(
        "single-deadline shutdown",
        elapsed,
        Duration::from_millis(560),
    );
    let _ = releaser.join();
    if !matches!(result, ck_gpu_metal::StoreCleanup::Unproven { .. })
        || elapsed > Duration::from_millis(560)
        || context.native_live_objects() == baseline
    {
        eprintln!(
            "single deadline: shutdown restarted its budget or released an outstanding graph"
        );
        return false;
    }
    if !matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    ) || !same_native_live_counts(context.native_live_objects(), baseline)
    {
        eprintln!("single deadline: later proof did not recover after completion");
        return false;
    }
    true
}

fn run_fatal_case(
    context: &Arc<MetalContext>,
    name: &str,
    hooks: ck_gpu_metal::TextureTestHooks,
    wakes: TextureWakes,
) -> bool {
    let (port, mut owner) = match TextureStore::start_with_test_hooks(
        Arc::clone(context),
        TextureLimits::default(),
        wakes,
        hooks,
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("{name}: start failed: {error}");
            return false;
        }
    };
    if port
        .offer(UploadItem {
            tag: 61_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![61; 4],
                drops: None,
            }),
        })
        .is_err()
    {
        eprintln!("{name}: offer refused");
        return false;
    }
    let until = Instant::now() + VERIFY_DEADLINE;
    let mut completed = Vec::new();
    while completed.is_empty() {
        owner.submit();
        owner.poll(&mut completed);
        if Instant::now() >= until {
            eprintln!("{name}: timed out");
            return false;
        }
        thread::yield_now();
    }
    if owner.take_failure().is_none() || owner.take_failure().is_some() {
        eprintln!("{name}: failure was not latched exactly once");
        return false;
    }
    if name.contains("lost wake") {
        let until = Instant::now() + Duration::from_secs(1);
        while owner.stats().wake_panics == 0 && Instant::now() < until {
            thread::yield_now();
        }
        if owner.stats().wake_panics != 1 {
            eprintln!("{name}: contained wake panic was not counted");
            return false;
        }
    }
    if !completed
        .iter()
        .all(|item| matches!(item, Completed::Failed { .. }))
    {
        eprintln!("{name}: failed batch produced a ready texture");
        return false;
    }
    if port
        .offer(UploadItem {
            tag: 62_u32,
            width: 1,
            height: 1,
            pixels: Arc::new(Pixels {
                bytes: vec![62; 4],
                drops: None,
            }),
        })
        .is_ok()
    {
        eprintln!("{name}: accepted an offer after fatal failure");
        return false;
    }
    if !matches!(
        owner.shutdown(Instant::now() + Duration::from_secs(3)),
        ck_gpu_metal::StoreCleanup::Proven
    ) {
        eprintln!("{name}: shutdown proof failed");
        return false;
    }
    true
}

fn main() -> ExitCode {
    if env::args().any(|arg| arg == WORKER) {
        return worker();
    }
    let current = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("supervisor: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut child = match Command::new(current)
        .arg(WORKER)
        .env("MTL_DEBUG_LAYER", "1")
        .env("MTL_SHADER_VALIDATION", "1")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("supervisor: could not spawn worker: {error}");
            return ExitCode::FAILURE;
        }
    };
    let until = Instant::now() + DEADLINE;
    loop {
        match child.try_wait() {
            Ok(Some(status))
                if status.code() == Some(77)
                    && env::var("CK_REQUIRE_FULL_SPEC").as_deref() != Ok("1") =>
            {
                println!(
                    "skip: no Metal GPU available (set CK_REQUIRE_FULL_SPEC=1 to require one)"
                );
                return ExitCode::SUCCESS;
            }
            Ok(Some(status)) if status.success() => return ExitCode::SUCCESS,
            Ok(Some(status)) => {
                eprintln!("supervisor: worker exited with {status}");
                return ExitCode::FAILURE;
            }
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let reap_until = Instant::now() + Duration::from_secs(2);
                while Instant::now() < reap_until {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                eprintln!("supervisor: worker exceeded {DEADLINE:?}");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("supervisor: worker wait failed: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

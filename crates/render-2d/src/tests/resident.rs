use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use ck_resources::{
    CpuPixels, Mode, ResourceLimits, ResourceService, StatusState, TextureKey, UploadRequest,
};

use crate::resident::{
    LedgerOps, ResidentAdapter, ResidentBoundary, ResidentEvent, ResidentRefusal, WakeRelay,
};

#[derive(Default)]
struct FakeLedger {
    room: usize,
    extra: usize,
    offered: Vec<UploadRequest>,
    events: VecDeque<ResidentEvent>,
    commits: Vec<(TextureKey, u64)>,
    rejects: Vec<(TextureKey, u64)>,
    adoptions: Vec<(TextureKey, u64)>,
    adopt_calls: usize,
    retirements: Vec<TextureKey>,
    refusal: Option<ResidentRefusal>,
    hold_refused_request: bool,
    pending_retirements: VecDeque<TextureKey>,
    pinned: HashSet<TextureKey>,
    active: HashMap<TextureKey, u64>,
    observed_adopt_limit: usize,
}

impl FakeLedger {
    fn pump(&mut self) {
        let count = self.pending_retirements.len();
        for _ in 0..count {
            if let Some(key) = self.pending_retirements.pop_front() {
                if self.pinned.contains(&key) {
                    self.pending_retirements.push_back(key);
                } else {
                    self.events.push_back(ResidentEvent::KeyRetired { key });
                }
            }
        }
    }
}

impl LedgerOps for FakeLedger {
    fn room(&self) -> usize {
        self.room
    }
    fn extra_room(&self) -> usize {
        self.extra
    }
    fn offer(&mut self, request: UploadRequest) -> Result<(), ResidentRefusal> {
        if let Some(refusal) = self.refusal.take() {
            if self.hold_refused_request {
                self.offered.push(request);
            }
            return Err(refusal);
        }
        self.offered.push(request);
        Ok(())
    }
    fn next_event(&mut self) -> Option<ResidentEvent> {
        self.events.pop_front()
    }
    fn has_events(&self) -> bool {
        !self.events.is_empty()
    }
    fn commit(&mut self, key: TextureKey, revision: u64) {
        self.commits.push((key, revision));
        if self.active.insert(key, revision).is_some() {
            self.pending_retirements.push_back(key);
        }
    }
    fn reject(&mut self, key: TextureKey, revision: u64) {
        self.rejects.push((key, revision));
        self.pending_retirements.push_back(key);
    }
    fn adopt(&mut self, limit: usize, out: &mut Vec<(TextureKey, u64)>) {
        self.adopt_calls += 1;
        self.observed_adopt_limit = limit;
        out.extend(self.adoptions.drain(..).take(limit));
    }
    fn retire_key(&mut self, key: TextureKey) {
        self.retirements.push(key);
        self.pending_retirements.push_back(key);
    }
}

fn new_service() -> ResourceService {
    ResourceService::new(ResourceLimits::DEFAULT, Mode::Backend).expect("backend service")
}

fn finished_resource(service: &ResourceService) -> TextureKey {
    let reservation = service.reserve_pixels(1, 1).expect("pixel reservation");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("valid pixels");
    let key = service
        .create_from_pixels(pixels)
        .expect("resource admission");
    let job = service.take_job().expect("queued image job");
    service.complete(job.run());
    key
}

fn fresh_pixels(service: &ResourceService) -> CpuPixels {
    CpuPixels::premultiplied(
        service.reserve_pixels(1, 1).expect("pixel reservation"),
        vec![0; 4],
    )
    .expect("valid pixels")
}

fn resource_retiring(service: &ResourceService) -> TextureKey {
    let key = service
        .create_from_pixels(fresh_pixels(service))
        .expect("resource admission");
    service.release(key).expect("release resource");
    let _ = service.maintain(1);
    key
}

#[test]
fn whole_path_offers_commits_then_adopts_only_at_a_frame_boundary() {
    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 4,
        extra: 4,
        ..FakeLedger::default()
    };

    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.offered.len(), 1);
    assert_eq!(service.status(key).unwrap().state, StatusState::Uploading);
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 1 });
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.commits, vec![(key, 1)]);
    ledger.adoptions.push((key, 1));
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(service.status(key).unwrap().state, StatusState::Uploading);
    adapter.pass(&mut ledger, ResidentBoundary::Frame);
    let status = service.status(key).unwrap();
    assert_eq!(status.state, StatusState::Ready);
    assert_eq!(
        status.shown.map(|shown| (shown.width, shown.height)),
        Some((1, 1))
    );
}

#[test]
fn upload_limit_is_the_minimum_of_both_ledger_rooms() {
    let service = new_service();
    let _first = finished_resource(&service);
    let _second = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service, 8);
    let mut ledger = FakeLedger {
        room: 3,
        extra: 1,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.offered.len(), 1);
    ledger.room = 0;
    ledger.extra = 4;
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.offered.len(), 1);
}

#[test]
fn room_zero_idles_until_the_room_wake_resumes_uploads() {
    let service = new_service();
    let _key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service, 8);
    let mut ledger = FakeLedger::default();
    let idle = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(!idle.more);
    assert!(ledger.offered.is_empty());
    ledger.room = 1;
    ledger.extra = 1;
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.offered.len(), 1);
}

#[test]
fn key_retiring_drop_returns_cpu_bytes_and_control_retains_them() {
    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 1,
        extra: 1,
        refusal: Some(ResidentRefusal::KeyRetiring),
        ..FakeLedger::default()
    };
    assert_eq!(service.stats().cpu_pixels.reserved, 4);
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(service.stats().cpu_pixels.reserved, 0);
    assert_eq!(adapter.stats().uploads_dropped_retiring, 1);
    assert_eq!(adapter.stats().protocol_errors, 0);
    assert!(service.status(key).unwrap().error.is_none());

    let controlled_key = finished_resource(&service);
    let mut controlled_adapter = ResidentAdapter::new(service.clone(), 8);
    let mut control = FakeLedger {
        room: 1,
        extra: 1,
        refusal: Some(ResidentRefusal::KeyRetiring),
        hold_refused_request: true,
        ..FakeLedger::default()
    };
    controlled_adapter.pass(&mut control, ResidentBoundary::Ordinary);
    assert_eq!(service.stats().cpu_pixels.reserved, 4);
    assert_eq!(
        service.status(controlled_key).unwrap().state,
        StatusState::Uploading,
        "a refusal ignored by the adapter would strand the resource"
    );
    assert!(service.status(controlled_key).unwrap().error.is_none());
    control.offered.clear();
    assert_eq!(service.stats().cpu_pixels.reserved, 0);
}

#[test]
fn protocol_refusal_becomes_a_visible_upload_failure() {
    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 1,
        extra: 1,
        refusal: Some(ResidentRefusal::Protocol("forced refusal".to_owned())),
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    let status = service.status(key).unwrap();
    assert_eq!(status.state, StatusState::Failed);
    assert!(
        status
            .error
            .unwrap()
            .message()
            .contains("refused an admitted upload")
    );
    assert_eq!(adapter.stats().protocol_errors, 1);
}

#[test]
fn reload_before_commit_rejects_stale_revision_but_after_commit_keeps_it_visible() {
    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 2,
        extra: 2,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    service
        .replace_pixels(key, fresh_pixels(&service))
        .expect("reload");
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 1 });
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.rejects, vec![(key, 1)]);

    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 2,
        extra: 2,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 1 });
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    service
        .replace_pixels(key, fresh_pixels(&service))
        .expect("replacement after commit");
    ledger.adoptions.push((key, 1));
    adapter.pass(&mut ledger, ResidentBoundary::Frame);
    let status = service.status(key).expect("live resource");
    assert_eq!(status.requested, 2);
    assert_eq!(status.shown.map(|shown| shown.revision), Some(1));
}

#[test]
fn frame_adoption_obeys_the_configured_cap() {
    let service = new_service();
    let keys = (0..3)
        .map(|_| finished_resource(&service))
        .collect::<Vec<_>>();
    let mut adapter = ResidentAdapter::new(service.clone(), 2);
    let mut ledger = FakeLedger {
        room: 3,
        extra: 3,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    for &key in &keys {
        ledger
            .events
            .push_back(ResidentEvent::Prepared { key, revision: 1 });
    }
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    ledger.adoptions.extend(keys.iter().map(|&key| (key, 1)));
    adapter.pass(&mut ledger, ResidentBoundary::Frame);
    assert_eq!(ledger.observed_adopt_limit, 2);
    assert_eq!(adapter.stats().adoptions, 2);
    assert_eq!(service.status(keys[0]).unwrap().state, StatusState::Ready);
    assert_eq!(service.status(keys[1]).unwrap().state, StatusState::Ready);
    assert_eq!(
        service.status(keys[2]).unwrap().state,
        StatusState::Uploading
    );
}

#[test]
fn upload_failed_event_reaches_the_resource_status() {
    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 1,
        extra: 1,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    ledger.events.push_back(ResidentEvent::UploadFailed {
        key,
        revision: 1,
        reason: "simulated upload failure".to_owned(),
    });
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    let status = service.status(key).unwrap();
    assert_eq!(status.state, StatusState::Failed);
    assert_eq!(status.error.unwrap().message(), "simulated upload failure");
}

#[test]
fn service_retirement_runs_without_another_caller_action() {
    let service = new_service();
    let key = resource_retiring(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger::default();
    let first = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(first.more);
    assert_eq!(ledger.retirements, vec![key]);
    ledger.pump();
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    let _ = service.maintain(8);
    assert!(
        service.status(key).is_err(),
        "retirement proof frees the slot"
    );
    assert_eq!(adapter.stats().retirements, 1);
}

#[test]
fn thousand_retirements_rearm_at_256_and_a_no_rearm_control_strands_744() {
    let service = new_service();
    for _ in 0..1_000 {
        resource_retiring(&service);
    }
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger::default();
    let first = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(first.more);
    assert_eq!(ledger.retirements.len(), 256);
    assert_eq!(
        1_000 - ledger.retirements.len(),
        744,
        "no-rearm control remainder"
    );
    let mut outcome = first;
    for _ in 0..8 {
        if !outcome.more {
            break;
        }
        ledger.pump();
        outcome = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    }
    assert!(!outcome.more);
    assert_eq!(ledger.retirements.len(), 1_000);
    let _ = service.maintain(usize::MAX);
    assert_eq!(service.stats().registry.live, 0);
}

#[test]
fn post_pump_retirement_rearms_for_retire_reject_and_delayed_pin() {
    let service = new_service();
    let key = resource_retiring(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        active: HashMap::from([(key, 1)]),
        ..FakeLedger::default()
    };
    let first = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(first.more, "retire_key creates work for the next pump");
    ledger.pump();
    assert_eq!(
        ledger.events.front(),
        Some(&ResidentEvent::KeyRetired { key })
    );
    let relayed = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(!relayed.more);
    let _ = service.maintain(8);
    assert!(service.status(key).is_err());

    // Negative control: if the scheduler ignores `more`, the next pump never runs.
    let service = new_service();
    let stranded_key = resource_retiring(&service);
    let mut stranded_adapter = ResidentAdapter::new(service, 8);
    let mut stranded_ledger = FakeLedger {
        active: HashMap::from([(stranded_key, 1)]),
        ..FakeLedger::default()
    };
    let stranded = stranded_adapter.pass(&mut stranded_ledger, ResidentBoundary::Ordinary);
    assert!(stranded.more);
    assert!(stranded_ledger.events.is_empty());
    assert_eq!(stranded_ledger.pending_retirements, [stranded_key]);

    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 1,
        extra: 1,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    service
        .replace_pixels(key, fresh_pixels(&service))
        .expect("replacement");
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 1 });
    let rejected = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(rejected.more, "reject creates work for the next pump");
    ledger.pump();
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert_eq!(ledger.rejects, vec![(key, 1)]);

    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 1,
        extra: 1,
        ..FakeLedger::default()
    };
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 1 });
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    service
        .replace_pixels(key, fresh_pixels(&service))
        .expect("replacement");
    let job = service.take_job().expect("replacement job");
    service.complete(job.run());
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 2 });
    let committed = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(
        committed.more,
        "superseding commit conservatively rearms the next pump"
    );
    ledger.pinned.insert(key);
    ledger.pump();
    let pinned = adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(!pinned.more, "a pinned revision waits for token completion");
    ledger.pinned.remove(&key);
    ledger.pump();
    adapter.pass(&mut ledger, ResidentBoundary::Ordinary);
    assert!(ledger.events.is_empty());
}

#[test]
fn ending_detaches_before_handling_events_or_taking_waiting_uploads() {
    let service = new_service();
    let key = finished_resource(&service);
    let mut adapter = ResidentAdapter::new(service.clone(), 8);
    let mut ledger = FakeLedger {
        room: 1,
        extra: 1,
        ..FakeLedger::default()
    };
    ledger
        .events
        .push_back(ResidentEvent::Prepared { key, revision: 1 });
    adapter.pass(&mut ledger, ResidentBoundary::Ending);
    assert!(ledger.offered.is_empty());
    assert!(ledger.commits.is_empty());
    assert_eq!(ledger.rejects, vec![(key, 1)]);
    assert_eq!(service.stats().cpu_pixels.reserved, 0);
    ledger.events.push_back(ResidentEvent::KeyRetired { key });
    ledger.adoptions.push((key, 1));
    adapter.pass(&mut ledger, ResidentBoundary::Frame);
    assert!(ledger.offered.is_empty());
    assert_eq!(ledger.adoptions, vec![(key, 1)]);
    assert_eq!(ledger.adopt_calls, 0);

    // The r1-order control offers the service upload before detaching.
    let service = new_service();
    let key = finished_resource(&service);
    let mut controlled = FakeLedger {
        room: 1,
        extra: 1,
        ..FakeLedger::default()
    };
    let mut leaked = Vec::new();
    service.take_uploads(&mut leaked, 1);
    for request in leaked.drain(..) {
        controlled
            .offer(request)
            .expect("the broken order offers before detach");
    }
    service.detach_backend();
    assert_eq!(controlled.offered.len(), 1);
    assert_eq!(controlled.offered[0].key, key);
}

#[test]
fn wake_relay_delivers_preconnect_work_once_and_disconnects_future_calls() {
    let relay = WakeRelay::default();
    let wake = relay.wake();
    wake();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    relay.connect(Arc::new(move || {
        seen.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    wake();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    relay.disconnect();
    wake();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn wake_callback_can_reenter_disconnect_and_a_copied_callback_may_finish_once() {
    let relay = WakeRelay::default();
    let wake = relay.wake();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (continue_tx, continue_rx) = mpsc::sync_channel(1);
    let continue_rx = Arc::new(std::sync::Mutex::new(continue_rx));
    let callback_continue_rx = Arc::clone(&continue_rx);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let callback_relay = relay.clone();
    relay.connect(Arc::new(move || {
        seen.fetch_add(1, Ordering::SeqCst);
        entered_tx.send(()).expect("test receiver remains open");
        callback_continue_rx
            .lock()
            .expect("receiver lock remains healthy")
            .recv_timeout(Duration::from_secs(2))
            .expect("test releases the in-flight callback");
        callback_relay.disconnect();
    }));
    let wake_thread = std::thread::spawn(move || wake());
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("callback starts outside the relay lock");
    relay.disconnect();
    continue_tx.send(()).expect("callback is waiting");
    wake_thread.join().expect("copied callback completes");
    (relay.wake())();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn event_bound_rearms_a_second_pass_without_losing_events() {
    let service = new_service();
    let mut adapter = ResidentAdapter::new(service, 8);
    let mut ledger = FakeLedger::default();
    for _ in 0..1_025 {
        ledger.events.push_back(ResidentEvent::GpuFailure {
            message: "test".to_owned(),
        });
    }
    assert!(adapter.pass(&mut ledger, ResidentBoundary::Ordinary).more);
    assert!(ledger.events.is_empty());
    assert!(!adapter.pass(&mut ledger, ResidentBoundary::Ordinary).more);
    assert!(ledger.events.is_empty());
}

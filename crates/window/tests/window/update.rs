//! Paced UI updates use a fake monotonic clock and the same loop/backend seams as production.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, atomic::AtomicU64};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{
    Answer, Clock, CloseSource, LoopEnd, LoopMetrics, LoopSettings, MetricsWindow, NativeEvent,
    PixelSize, PresenterReport, UiAnswer, UiEvent, UpdateEvent, UpdateSettings, Wait,
};

use crate::fakes::{DEADLINE, Rig, lock, owned, scenario};

#[derive(Clone)]
struct FakeClock {
    base: Instant,
    nanos: Arc<AtomicU64>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            nanos: Arc::new(AtomicU64::new(0)),
        }
    }

    fn set(&self, at: Instant) {
        let nanos = at.saturating_duration_since(self.base).as_nanos();
        self.nanos.store(
            u64::try_from(nanos).expect("test instant fits"),
            Ordering::SeqCst,
        );
    }

    fn after(&self, duration: Duration) -> Instant {
        self.base.checked_add(duration).expect("test instant fits")
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base
            .checked_add(Duration::from_nanos(self.nanos.load(Ordering::SeqCst)))
            .expect("test clock instant fits")
    }
}

fn extract_updates(posts: &[UiEvent]) -> Vec<UpdateEvent> {
    posts
        .iter()
        .filter_map(|event| match event {
            UiEvent::Update(update) => Some(update.clone()),
            _ => None,
        })
        .collect()
}

fn configure_updates(
    rig: &mut Rig,
    clock: &FakeClock,
    rate: u32,
    max_delta: Duration,
) -> ck_window::UpdateControl {
    let settings = UpdateSettings::new(rate, max_delta).expect("valid update settings");
    let control = settings.control();
    control.set(true);
    rig.settings = LoopSettings::default().with_updates(settings);
    rig.backend.deadline_supported = true;
    let clock = clock.clone();
    rig.backend.deadline_hook = Some(Box::new(move |deadline| clock.set(deadline)));
    lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
    control
}

fn quit_after_updates(rig: &mut Rig, count: usize) {
    let ui = Arc::clone(&rig.ui);
    let quit = Arc::clone(&rig.quit);
    let notifier = Arc::clone(&rig.notifier);
    rig.backend.hook = Some(Box::new(move |_| {
        if extract_updates(&ui.posted()).len() >= count {
            quit.store(true, Ordering::SeqCst);
            notifier.wake();
        }
    }));
}

#[test]
fn update_settings_validate_rate_and_representable_periods() {
    assert_eq!(
        UpdateSettings::new(1, Duration::from_millis(1))
            .expect("1 Hz")
            .period(),
        Duration::from_secs(1)
    );
    assert_eq!(
        UpdateSettings::new(1_000, Duration::from_millis(1))
            .expect("1,000 Hz")
            .period(),
        Duration::from_millis(1)
    );
    assert_eq!(
        UpdateSettings::new(0, Duration::from_millis(1))
            .expect_err("zero rate")
            .setting,
        "updates_per_second"
    );
    assert_eq!(
        UpdateSettings::new(1_001, Duration::from_millis(1))
            .expect_err("rate above limit")
            .setting,
        "updates_per_second"
    );
    assert_eq!(
        UpdateSettings::new(60, Duration::ZERO)
            .expect_err("zero delta")
            .setting,
        "max_delta"
    );
}

#[test]
fn unsupported_backend_refuses_updates_before_window_creation_or_pumping() {
    owned(
        module_path!(),
        "unsupported_backend_refuses_updates_before_window_creation_or_pumping",
        DEADLINE,
        || {
            let _scenario = scenario();
            let mut rig = Rig::new();
            rig.settings = LoopSettings::default().with_updates(
                UpdateSettings::new(60, Duration::from_millis(100)).expect("valid settings"),
            );
            let ran = rig.run_collect();
            assert_eq!(
                ran.end,
                LoopEnd::Failed {
                    message: "the update scheduler needs timed waits, which this backend does not provide"
                        .to_owned()
                }
            );
            assert!(!ran.log.iter().any(|line| line == "backend.create_window"));
            assert!(!ran.log.iter().any(|line| line.starts_with("backend.pump")));
        },
    );
}

#[test]
fn active_updates_use_the_deadline_grid_ack_input_and_bound_delta() {
    owned(
        module_path!(),
        "active_updates_use_the_deadline_grid_ack_input_and_bound_delta",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let (ran, posts, deadlines) = {
                let mut rig = Rig::new();
                let update_settings =
                    UpdateSettings::new(60, Duration::from_millis(20)).expect("valid settings");
                let control = update_settings.control();
                control.set(true);
                rig.settings = LoopSettings::default().with_updates(update_settings);
                rig.backend.deadline_supported = true;
                let fake_time = clock.clone();
                rig.backend.deadline_hook = Some(Box::new(move |deadline| fake_time.set(deadline)));
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.backend.script.push_back(vec![NativeEvent::Key {
                    key: ck_window::Key::ArrowLeft,
                    down: true,
                    repeat: false,
                }]);
                lock(&rig.ui.update_answers).extend(std::iter::repeat_n(Answer::Returned, 3));
                let ui = Arc::clone(&rig.ui);
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |_| {
                    if extract_updates(&ui.posted()).len() >= 3 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ui = Arc::clone(&rig.ui);
                let pumps = Arc::clone(&rig.backend.pumps);
                let ran = rig.run_collect_with_clock(&clock);
                (ran, ui.posted(), lock(&pumps).clone())
            };
            let sent = extract_updates(&posts);
            assert_eq!(sent.len(), 3);
            assert_eq!(
                sent.iter().map(|event| event.sequence).collect::<Vec<_>>(),
                [1, 2, 3]
            );
            assert_eq!(sent[0].elapsed, Duration::ZERO);
            assert_eq!(sent[0].dt, Duration::ZERO);
            assert_eq!(sent[1].elapsed, Duration::from_nanos(16_666_666));
            assert_eq!(sent[2].elapsed, Duration::from_nanos(16_666_666));
            assert!(sent[0].input.transitions.len() > sent[1].input.transitions.len());
            assert!(sent[0].input.actions.iter().any(|action| action.pressed));
            assert!(sent.iter().all(|event| event.metrics.points == (400, 300)));
            assert!(sent.iter().all(|event| event.metrics.pixels
                == PixelSize {
                    width: 800,
                    height: 600
                }));
            assert!(
                deadlines
                    .iter()
                    .filter(|call| call.deadline.is_some())
                    .count()
                    >= 2
            );
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn a_delayed_answer_coalesces_missed_ticks_into_one_update_with_elapsed_delta() {
    owned(
        module_path!(),
        "a_delayed_answer_coalesces_missed_ticks_into_one_update_with_elapsed_delta",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let (ran, posts) = {
                let mut rig = Rig::new();
                let updates_cfg =
                    UpdateSettings::new(60, Duration::from_millis(40)).expect("valid settings");
                updates_cfg.control().set(true);
                rig.settings = LoopSettings::default().with_updates(updates_cfg);
                rig.backend.deadline_supported = true;
                let time = clock.clone();
                rig.backend.deadline_hook = Some(Box::new(move |deadline| time.set(deadline)));
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                let ui = Arc::clone(&rig.ui);
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                let time = clock.clone();
                let driver = thread::spawn(move || {
                    let first = wait_for_update(&ui, 1);
                    time.set(time.after(Duration::from_millis(100)));
                    ui.answer(UiEvent::Update(first), Answer::Returned);
                    let second = wait_for_update(&ui, 2);
                    assert_eq!(second.sequence, 2, "missed opportunities were not replayed");
                    assert_eq!(second.elapsed, Duration::from_millis(100));
                    assert_eq!(second.dt, Duration::from_millis(40));
                    ui.answer(UiEvent::Update(second), Answer::Returned);
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                });
                let posts_ui = Arc::clone(&rig.ui);
                let ran = rig.run_collect_with_clock(&clock);
                driver.join().expect("answer driver completed");
                (ran, posts_ui.posted())
            };
            assert_eq!(
                extract_updates(&posts).len(),
                2,
                "one pending opportunity follows the delayed answer"
            );
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

/// The loop's measurement window counts what the scheduler did, by timestamps (CK-129): a delayed answer at 60 Hz coalesces the
/// five opportunities that fell due between the first update and the answer (at 1/60 s to 5/60 s) into one later update, so the
/// window holds two delivered updates and five missed opportunities, all admitted by their own times inside `[start, end)`.
/// Negative control: counting at the answer (not at the due times), or counting the replayed update as missed, changes the five.
#[test]
fn a_delayed_answer_counts_the_coalesced_opportunities_and_the_delivered_updates_in_the_window() {
    owned(
        module_path!(),
        "a_delayed_answer_counts_the_coalesced_opportunities_and_the_delivered_updates_in_the_window",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            // The instrumentation's clock reads one second at the loop's first boundary; the fake clock's instants map from there.
            let metrics = LoopMetrics::new(Arc::new(|| 1_000_000_000));
            metrics
                .request(MetricsWindow {
                    id: 1,
                    start: 1_000_000_000,
                    end: Some(2_000_000_000),
                })
                .expect("a first request");
            let mut rig = Rig::new();
            let updates_cfg = UpdateSettings::new(60, Duration::from_millis(40))
                .expect("valid settings")
                .with_metrics(metrics.clone());
            updates_cfg.control().set(true);
            rig.settings = LoopSettings::default().with_updates(updates_cfg);
            rig.backend.deadline_supported = true;
            let time = clock.clone();
            rig.backend.deadline_hook = Some(Box::new(move |deadline| time.set(deadline)));
            lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            let time = clock.clone();
            let driver = thread::spawn(move || {
                let first = wait_for_update(&ui, 1);
                time.set(time.after(Duration::from_millis(100)));
                ui.answer(UiEvent::Update(first), Answer::Returned);
                let second = wait_for_update(&ui, 2);
                ui.answer(UiEvent::Update(second), Answer::Returned);
                quit.store(true, Ordering::SeqCst);
                notifier.wake();
            });
            let ran = rig.run_collect_with_clock(&clock);
            driver.join().expect("answer driver completed");
            assert_eq!(ran.end, LoopEnd::Quit);
            let window = metrics.snapshot(1).expect("the window is kept").window;
            assert_eq!(
                window.adopted_at,
                Some(1_000_000_000),
                "adopted at the loop's first boundary"
            );
            assert!(!window.late_adopted);
            assert_eq!(window.stats.delivered, 2, "{window:?}");
            assert_eq!(window.stats.missed, 5, "{window:?}");
            assert_eq!(window.outside, 0, "everything lies inside the window");
        },
    );
}

/// The loop's progress fence (CK-129 review F2): the opportunities that fell due after the loop's last pass and before the window's end
/// are counted when the teardown examines the schedule once more, and the window is not final until then. The pump hook ends the run;
/// the presenter's stop, where the application takes the window's end, finds the clock 100 ms past the loop's last examination of its
/// schedule, and the five opportunities due at 1/60 s .. 5/60 s lie in the window `[10 ms, 90 ms)`: only the teardown's examination
/// can count them. Before it, a read past the end on the clock is still incomplete; after it, the counts are exact and stable.
#[test]
fn the_teardown_counts_the_opportunities_due_since_the_last_pass_and_the_window_is_final_only_then()
{
    owned(
        module_path!(),
        "the_teardown_counts_the_opportunities_due_since_the_last_pass_and_the_window_is_final_only_then",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let uptime = Arc::new(AtomicU64::new(1_000_000_000));
            let reader = Arc::clone(&uptime);
            let metrics = LoopMetrics::new(Arc::new(move || reader.load(Ordering::SeqCst)));
            metrics
                .request(MetricsWindow {
                    id: 1,
                    start: 1_010_000_000,
                    end: Some(1_090_000_000),
                })
                .expect("a request");
            let mut rig = Rig::new();
            let updates_cfg = UpdateSettings::new(60, Duration::from_millis(40))
                .expect("valid settings")
                .with_metrics(metrics.clone());
            updates_cfg.control().set(true);
            rig.settings = LoopSettings::default().with_updates(updates_cfg);
            rig.backend.deadline_supported = true;
            let time = clock.clone();
            rig.backend.deadline_hook = Some(Box::new(move |deadline| time.set(deadline)));
            lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            let time = clock.clone();
            let probe = metrics.clone();
            let during = Arc::new(Mutex::new(None));
            let seen = Arc::clone(&during);
            // The first update is posted and never answered. When the loop begins its teardown, the presenter's stop is where the
            // application takes the window's end: 100 ms have passed since the loop's last pass, and the instrumentation's clock is
            // past the window's end. The window is read from inside the stop, before the loop has examined its schedule again.
            rig.presenter.on_stop = Some(Box::new(move || {
                time.set(time.after(Duration::from_millis(100)));
                uptime.store(2_000_000_000, Ordering::SeqCst);
                let reading = probe.read(1, Duration::from_millis(30)).expect("kept");
                *lock(&seen) = Some((reading.complete, reading.window.stats.missed));
            }));
            rig.backend.hook = Some(Box::new(move |_| {
                if !extract_updates(&ui.posted()).is_empty() {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(ran.end, LoopEnd::Quit);
            let (complete_before, missed_before) = lock(&during).take().expect("the hook ran");
            assert!(
                !complete_before,
                "past the end on the clock but the loop has not accounted through it"
            );
            assert_eq!(
                missed_before, 0,
                "the last pass only captured the opportunity at 1/60 s, which is missed only once a later one replaces it"
            );
            let reading = metrics.read(1, Duration::from_millis(30)).expect("kept");
            assert!(reading.complete, "the teardown accounted through the end");
            assert_eq!(
                reading.window.stats.missed, 5,
                "1/60 s .. 5/60 s, each by its own due time: {:?}",
                reading.window
            );
            assert_eq!(
                reading.window.stats.delivered, 0,
                "the one update was posted at 0, before the window"
            );
            let again = metrics.read(1, Duration::from_millis(30)).expect("kept");
            assert_eq!(again.window.stats.missed, 5, "stable once final");
        },
    );
}

#[test]
fn equal_held_action_displacement_is_independent_of_the_update_rate() {
    owned(
        module_path!(),
        "equal_held_action_displacement_is_independent_of_the_update_rate",
        DEADLINE,
        || {
            let mut displacements = Vec::new();
            for rate in [30, 60, 120] {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, rate, Duration::from_millis(100));
                rig.backend.script.push_back(vec![NativeEvent::Key {
                    key: ck_window::Key::ArrowLeft,
                    down: true,
                    repeat: false,
                }]);
                let updates = usize::try_from(rate).expect("rate fits") + 1;
                lock(&rig.ui.update_answers).extend(std::iter::repeat_n(Answer::Returned, updates));
                quit_after_updates(&mut rig, updates);
                let posts = rig.run_collect_with_clock(&clock).posts;
                let sent = extract_updates(&posts);
                assert_eq!(sent.len(), updates);
                assert!(
                    sent[0].input.actions.iter().any(|action| {
                        action.name == "move_left" && action.held && action.pressed
                    })
                );
                let position = sent
                    .iter()
                    .filter(|update| {
                        update
                            .input
                            .actions
                            .iter()
                            .any(|action| action.name == "move_left" && action.held)
                    })
                    .map(|update| 3.5 * update.dt.as_secs_f64())
                    .sum::<f64>();
                displacements.push((rate, position));
            }
            let (slow_rate, slow) = displacements[0];
            for (rate, position) in displacements.into_iter().skip(1) {
                let tolerance = 3.5 / f64::from(rate);
                assert!(
                    (position - slow).abs() <= tolerance,
                    "held action displacement differs at {slow_rate} Hz ({slow}) and {rate} Hz ({position})"
                );
            }
            assert!(
                (slow - 3.5).abs() < 3.5 / 30.0,
                "one second at 30 Hz: {slow}"
            );
        },
    );
}

#[test]
fn focus_minimize_and_zero_size_pause_updates_and_clear_held_input() {
    owned(
        module_path!(),
        "focus_minimize_and_zero_size_pause_updates_and_clear_held_input",
        DEADLINE,
        || {
            let pauses = [
                (NativeEvent::FocusLost, NativeEvent::FocusGained),
                (NativeEvent::Minimized, NativeEvent::Restored),
                (
                    NativeEvent::PixelSizeChanged {
                        width: 0,
                        height: 0,
                    },
                    NativeEvent::PixelSizeChanged {
                        width: 800,
                        height: 600,
                    },
                ),
            ];
            for (pause, resume) in pauses {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
                rig.backend.script.extend([
                    vec![NativeEvent::Key {
                        key: ck_window::Key::ArrowLeft,
                        down: true,
                        repeat: false,
                    }],
                    vec![pause],
                    vec![resume],
                ]);
                lock(&rig.ui.update_answers).extend([Answer::Returned, Answer::Returned]);
                let fake_clock = clock.clone();
                let ui = Arc::clone(&rig.ui);
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 3 {
                        fake_clock.set(fake_clock.after(Duration::from_secs(5)));
                    }
                    if extract_updates(&ui.posted()).len() >= 2 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ran = rig.run_collect_with_clock(&clock);
                let sent = extract_updates(&ran.posts);
                assert_eq!(sent.len(), 2, "a paused interval admitted no update");
                assert_eq!(sent[0].dt, Duration::ZERO);
                assert_eq!(sent[1].dt, Duration::ZERO, "resume has a fresh time origin");
                assert!(sent[1].input.reset, "resume reports the input reset");
                assert!(sent[1].input.transitions.iter().any(|transition| {
                    transition.action == "move_left" && transition.edge == ck_input::Edge::Release
                }));
                assert!(sent[1].input.actions.iter().any(|action| {
                    action.name == "move_left" && !action.held && action.released
                }));
                assert_eq!(ran.end, LoopEnd::Quit);
            }
        },
    );
}

#[cfg(feature = "test-support")]
#[test]
fn opted_in_test_worker_keeps_updates_after_focus_loss_and_input_still_resets() {
    owned(
        module_path!(),
        "opted_in_test_worker_keeps_updates_after_focus_loss_and_input_still_resets",
        DEADLINE,
        || {
            let _scenario = scenario();
            let _unfocused = ck_window::test_support::allow_updates_while_unfocused();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            rig.backend.script.push_back(vec![
                NativeEvent::Key {
                    key: ck_window::Key::ArrowLeft,
                    down: true,
                    repeat: false,
                },
                NativeEvent::FocusLost,
            ]);
            lock(&rig.ui.update_answers).push_back(Answer::Returned);
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            rig.backend.hook = Some(Box::new(move |_| {
                if extract_updates(&ui.posted()).len() == 1 {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));

            let ran = rig.run_collect_with_clock(&clock);
            let updates = extract_updates(&ran.posts);
            assert_eq!(updates.len(), 1);
            assert!(updates[0].input.reset, "FocusLost still resets held input");
            assert!(updates[0].input.actions.iter().all(|action| !action.held));
            assert_eq!(ran.end, LoopEnd::Quit);

            let mut minimized = Rig::new();
            let minimized_clock = FakeClock::new();
            configure_updates(
                &mut minimized,
                &minimized_clock,
                60,
                Duration::from_millis(100),
            );
            minimized
                .backend
                .script
                .push_back(vec![NativeEvent::Minimized]);
            let quit = Arc::clone(&minimized.quit);
            let notifier = Arc::clone(&minimized.notifier);
            minimized.backend.hook = Some(Box::new(move |call| {
                if call.wait == Wait::UntilEvent {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));
            let minimized_run = minimized.run_collect_with_clock(&minimized_clock);
            assert!(extract_updates(&minimized_run.posts).is_empty());
            assert!(minimized_run.waits.contains(&Wait::UntilEvent));
            assert_eq!(minimized_run.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn full_update_post_keeps_input_edges_until_the_successful_snapshot_is_acknowledged() {
    owned(
        module_path!(),
        "full_update_post_keeps_input_edges_until_the_successful_snapshot_is_acknowledged",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            rig.backend.script.extend([
                vec![NativeEvent::Key {
                    key: ck_window::Key::ArrowLeft,
                    down: true,
                    repeat: false,
                }],
                vec![NativeEvent::Key {
                    key: ck_window::Key::ArrowLeft,
                    down: false,
                    repeat: false,
                }],
            ]);
            rig.ui.full_update_posts.store(1, Ordering::SeqCst);
            lock(&rig.ui.update_answers).extend([Answer::Returned, Answer::Returned]);
            quit_after_updates(&mut rig, 2);
            let posts = rig.run_collect_with_clock(&clock).posts;
            let sent = extract_updates(&posts);
            assert_eq!(sent.len(), 2);
            assert!(
                sent[0].input.actions.iter().any(|action| {
                    action.name == "move_left" && action.pressed && action.released
                })
            );
            assert_eq!(sent[0].input.transitions.len(), 2);
            assert!(sent[1].input.transitions.is_empty());
            assert!(sent[1].input.actions.iter().all(|action| {
                action.name != "move_left" || (!action.pressed && !action.released)
            }));
        },
    );
}

#[test]
fn close_is_offered_before_a_due_update_and_full_close_blocks_that_pass_update() {
    owned(
        module_path!(),
        "close_is_offered_before_a_due_update_and_full_close_blocks_that_pass_update",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
            rig.ui.full_close_posts.store(1, Ordering::SeqCst);
            lock(&rig.ui.close_answers).push_back(Answer::Decline);
            lock(&rig.ui.update_answers).push_back(Answer::Returned);
            quit_after_updates(&mut rig, 1);
            let ran = rig.run_collect_with_clock(&clock);
            let close = ran
                .posts
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        UiEvent::CloseRequested {
                            source: CloseSource::Button
                        }
                    )
                })
                .expect("close callback posted");
            let update = ran
                .posts
                .iter()
                .position(|event| matches!(event, UiEvent::Update(_)))
                .expect("update eventually posted after close retry");
            assert!(
                close < update,
                "close precedes the due update: {:?}",
                ran.posts
            );
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn not_registered_suppresses_only_the_current_epoch_and_failure_ends_the_loop() {
    owned(
        module_path!(),
        "not_registered_suppresses_only_the_current_epoch_and_failure_ends_the_loop",
        DEADLINE,
        || {
            {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
                lock(&rig.ui.update_answers).push_back(Answer::NotRegistered);
                let ui = Arc::clone(&rig.ui);
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |_| {
                    if ui.answers_taken.load(Ordering::SeqCst) > 0 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ran = rig.run_collect_with_clock(&clock);
                assert_eq!(extract_updates(&ran.posts).len(), 1);
                assert_eq!(ran.end, LoopEnd::Quit);
            }
            {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
                lock(&rig.ui.update_answers).push_back(Answer::Failed {
                    message: "frame callback failed".to_owned(),
                });
                let ran = rig.run_collect_with_clock(&clock);
                assert_eq!(
                    ran.end,
                    LoopEnd::CallbackFailed {
                        message: "frame callback failed".to_owned()
                    }
                );
            }
        },
    );
}

#[test]
fn enabling_during_an_idle_wait_wakes_the_loop_and_posts_a_zero_delta_update() {
    owned(
        module_path!(),
        "enabling_during_an_idle_wait_wakes_the_loop_and_posts_a_zero_delta_update",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            let control = configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            control.set(false);
            let native = Arc::clone(&rig.backend.native);
            let ui = Arc::clone(&rig.ui);
            let driver_ui = Arc::clone(&ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            let driver = thread::spawn(move || {
                native.wait_sleeping(1);
                control.set(true);
                let first = wait_for_update(&driver_ui, 1);
                assert_eq!(first.dt, Duration::ZERO);
                quit.store(true, Ordering::SeqCst);
                notifier.wake();
            });
            let ran = rig.run_collect_with_clock(&clock);
            driver.join().expect("idle-wait driver completed");
            assert_eq!(extract_updates(&ran.posts).len(), 1);
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn a_control_change_at_the_sleep_boundary_is_not_lost() {
    owned(
        module_path!(),
        "a_control_change_at_the_sleep_boundary_is_not_lost",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            let control = configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            control.set(false);
            let control_at_wait = control.clone();
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            let mut changed = false;
            rig.backend.hook = Some(Box::new(move |call| {
                if !changed && call.wait == Wait::UntilEvent {
                    // The fake backend invokes this after the loop selected its idle wait and before
                    // it checks for a queued wake, exercising the sticky-wake side of the boundary.
                    changed = true;
                    control_at_wait.set(true);
                }
                if !extract_updates(&ui.posted()).is_empty() {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));
            let ran = rig.run_collect_with_clock(&clock);
            let updates = extract_updates(&ran.posts);
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].dt, Duration::ZERO);
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn a_stale_not_registered_answer_does_not_suppress_a_new_epoch() {
    owned(
        module_path!(),
        "a_stale_not_registered_answer_does_not_suppress_a_new_epoch",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            let control = configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            let answer_driver = thread::spawn(move || {
                let first = wait_for_update(&ui, 1);
                // Queue the old answer without waking. The registration epoch advances before
                // the loop can consume it, so this specifically tests stale-answer suppression.
                lock(&ui.answers).push_back(UiAnswer {
                    event: UiEvent::Update(first.clone()),
                    answer: Answer::NotRegistered,
                });
                control.set(true);
                let second = wait_for_update(&ui, 2);
                assert_eq!(second.sequence, first.sequence + 1);
                assert_eq!(second.dt, Duration::ZERO);
                quit.store(true, Ordering::SeqCst);
                notifier.wake();
            });
            let ran = rig.run_collect_with_clock(&clock);
            answer_driver.join().expect("stale answer driver completed");
            assert_eq!(extract_updates(&ran.posts).len(), 2);
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn disable_drops_a_pending_opportunity_and_reenable_uses_a_fresh_origin() {
    owned(
        module_path!(),
        "disable_drops_a_pending_opportunity_and_reenable_uses_a_fresh_origin",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            let control = configure_updates(&mut rig, &clock, 60, Duration::from_millis(100));
            let native = Arc::clone(&rig.backend.native);
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            let run_clock = clock.clone();
            let driver_clock = clock.clone();
            let driver = thread::spawn(move || {
                let first = wait_for_update(&ui, 1);
                control.set(false);
                native.wait_sleeping(1);
                driver_clock.set(driver_clock.after(Duration::from_secs(5)));
                control.set(true);
                // Answer the still-outstanding old update only after the new registration is live.
                ui.answer(UiEvent::Update(first), Answer::Returned);
                let next = wait_for_update(&ui, 2);
                assert_eq!(next.dt, Duration::ZERO);
                quit.store(true, Ordering::SeqCst);
                notifier.wake();
            });
            let ran = rig.run_collect_with_clock(&run_clock);
            driver.join().expect("disable/re-enable driver completed");
            assert_eq!(extract_updates(&ran.posts).len(), 2);
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

#[test]
fn a_configured_but_disabled_or_unfocused_source_does_not_make_timed_waits() {
    owned(
        module_path!(),
        "a_configured_but_disabled_or_unfocused_source_does_not_make_timed_waits",
        DEADLINE,
        || {
            for focus_loss in [false, true] {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                let settings = UpdateSettings::new(60, Duration::from_millis(100))
                    .expect("valid update settings");
                if focus_loss {
                    settings.control().set(true);
                    rig.backend.script.push_back(vec![NativeEvent::FocusLost]);
                    lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                }
                rig.settings = LoopSettings::default().with_updates(settings);
                rig.backend.deadline_supported = true;
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.wait == Wait::UntilEvent {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ran = rig.run_collect_with_clock(&clock);
                assert!(extract_updates(&ran.posts).is_empty());
                assert!(ran.deadlines.iter().all(Option::is_none));
                assert!(ran.waits.contains(&Wait::UntilEvent));
                assert_eq!(ran.end, LoopEnd::Quit);
            }
        },
    );
}

fn wait_for_update(ui: &crate::fakes::UiHandle, ordinal: usize) -> UpdateEvent {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(update) = extract_updates(&ui.posted()).get(ordinal - 1).cloned() {
            return update;
        }
        assert!(Instant::now() < deadline, "update {ordinal} was not posted");
        thread::yield_now();
    }
}

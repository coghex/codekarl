//! Paced UI updates use a fake monotonic clock and the same loop/backend seams as production.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, atomic::AtomicU64};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{
    Answer, Clock, CloseSource, LoopEnd, LoopMetrics, LoopSettings, MetricsWindow, NativeEvent,
    PixelSize, PointerEvent, PointerPosition, PresenterReport, UiAnswer, UiEvent, UpdateEvent,
    UpdateSettings, Wait,
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

/// The fake clock follows the loop's deadlines, except while an update is outstanding (posted, not yet answered): then it waits for
/// the test to move it, as the real clock would not run ahead of a test that is holding the UI.
fn hold_the_clock_while_an_update_is_outstanding(rig: &mut Rig, clock: &FakeClock) {
    let ui = Arc::clone(&rig.ui);
    let clock = clock.clone();
    rig.backend.deadline_hook = Some(Box::new(move |deadline| {
        let updates = extract_updates(&ui.posted()).len();
        let answered = ui.answered_updates.load(Ordering::SeqCst);
        if updates <= answered {
            clock.set(deadline);
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
            // The input is the tick's, consumed once: the first tick carries the press, the next ones nothing.
            assert!(
                ran.ticks.len() >= 3,
                "a tick per update: {}",
                ran.ticks.len()
            );
            assert!(ran.ticks[0].input.transitions.len() > ran.ticks[1].input.transitions.len());
            assert!(
                ran.ticks[0]
                    .input
                    .actions
                    .iter()
                    .any(|action| action.pressed)
            );
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
fn pointer_events_reach_one_scheduled_update_and_idle_state_does_not_repeat_them() {
    owned(
        module_path!(),
        "pointer_events_reach_one_scheduled_update_and_idle_state_does_not_repeat_them",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let (ran, posts) = {
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
                rig.backend.script.push_back(vec![
                    NativeEvent::Pointer(PointerEvent::Down(PointerPosition { x: 256, y: 512 })),
                    NativeEvent::Pointer(PointerEvent::Move(PointerPosition { x: 768, y: 1024 })),
                    NativeEvent::Pointer(PointerEvent::Up(PointerPosition { x: 768, y: 1024 })),
                ]);
                lock(&rig.ui.update_answers).extend([Answer::Returned, Answer::Returned]);
                quit_after_updates(&mut rig, 2);
                let ui = Arc::clone(&rig.ui);
                let ran = rig.run_collect_with_clock(&clock);
                (ran, ui.posted())
            };

            let updates = extract_updates(&posts);
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(updates.len(), 2);
            assert!(ran.ticks.len() >= 2, "{}", ran.ticks.len());
            assert_eq!(
                ran.ticks[0].input.pointer_events,
                [
                    PointerEvent::Down(PointerPosition { x: 256, y: 512 }),
                    PointerEvent::Move(PointerPosition { x: 768, y: 1024 }),
                    PointerEvent::Up(PointerPosition { x: 768, y: 1024 }),
                ]
            );
            assert!(ran.ticks[1].input.pointer_events.is_empty());
            assert_eq!(
                ran.ticks[1].input.pointer_position,
                Some(PointerPosition { x: 768, y: 1024 })
            );
            assert!(!ran.ticks[1].input.pointer_held);
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
                hold_the_clock_while_an_update_is_outstanding(&mut rig, &clock);
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
            hold_the_clock_while_an_update_is_outstanding(&mut rig, &clock);
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
                let ran = rig.run_collect_with_clock(&clock);
                let sent = extract_updates(&ran.posts);
                assert_eq!(sent.len(), updates);
                // The displacement is integrated over the input ticks (the map's input), at each tick's dt.
                assert!(
                    ran.ticks[0].input.actions.iter().any(|action| {
                        action.name == "move_left" && action.held && action.pressed
                    })
                );
                let position = ran
                    .ticks
                    .iter()
                    .filter(|tick| {
                        tick.input
                            .actions
                            .iter()
                            .any(|action| action.name == "move_left" && action.held)
                    })
                    .map(|tick| 3.5 * tick.dt.as_secs_f64())
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
                    NativeEvent::SizeChanged {
                        points: (0, 0),
                        pixels: PixelSize {
                            width: 0,
                            height: 0,
                        },
                    },
                    NativeEvent::SizeChanged {
                        points: (400, 300),
                        pixels: PixelSize {
                            width: 800,
                            height: 600,
                        },
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
                assert_eq!(ran.ticks.len(), 2, "a paused interval admitted no tick");
                assert_eq!(
                    ran.ticks[1].dt,
                    Duration::ZERO,
                    "resume has a fresh tick origin"
                );
                assert!(ran.ticks[1].input.reset, "resume reports the input reset");
                assert!(ran.ticks[1].input.transitions.iter().any(|transition| {
                    transition.action == "move_left" && transition.edge == ck_input::Edge::Release
                }));
                assert!(ran.ticks[1].input.actions.iter().any(|action| {
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
            assert!(
                ran.ticks[0].input.reset,
                "FocusLost still resets held input"
            );
            assert!(ran.ticks[0].input.actions.iter().all(|action| !action.held));
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
fn a_full_update_post_neither_repeats_nor_loses_input_edges_which_the_tick_consumed_once() {
    owned(
        module_path!(),
        "a_full_update_post_neither_repeats_nor_loses_input_edges_which_the_tick_consumed_once",
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
            let ran = rig.run_collect_with_clock(&clock);
            let sent = extract_updates(&ran.posts);
            assert_eq!(sent.len(), 2);
            // The refused post changes nothing about the input: each edge reaches the link at its own tick, exactly once and in
            // order, and the retried update carries no input of its own.
            let edges: Vec<(String, ck_input::Edge)> = ran
                .ticks
                .iter()
                .flat_map(|tick| tick.input.transitions.iter())
                .map(|transition| (transition.action.clone(), transition.edge))
                .collect();
            assert_eq!(
                edges,
                [
                    ("move_left".to_owned(), ck_input::Edge::Press),
                    ("move_left".to_owned(), ck_input::Edge::Release),
                ]
            );
            assert!(
                ran.ticks[0]
                    .input
                    .actions
                    .iter()
                    .any(|action| { action.name == "move_left" && action.pressed && action.held })
            );
            assert!(
                ran.ticks[1].input.actions.iter().any(|action| {
                    action.name == "move_left" && action.released && !action.held
                })
            );
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

// ---------------------------------------------------------------------------------------------
// The input tick (milestone 8, slice 0): its own cursor on the update grid, independent of the UI
// ---------------------------------------------------------------------------------------------

/// Quits the loop at the first pump that finds `count` input ticks consumed.
fn quit_after_ticks(rig: &mut Rig, count: usize) {
    let ui = Arc::clone(&rig.ui);
    let quit = Arc::clone(&rig.quit);
    let notifier = Arc::clone(&rig.notifier);
    rig.backend.hook = Some(Box::new(move |_| {
        if ui.ticks().len() >= count {
            quit.store(true, Ordering::SeqCst);
            notifier.wake();
        }
    }));
}

fn left_press() -> NativeEvent {
    NativeEvent::Key {
        key: ck_window::Key::ArrowLeft,
        down: true,
        repeat: false,
    }
}

#[test]
fn a_tick_runs_at_each_due_opportunity_while_an_update_is_outstanding_and_a_press_arrives_once() {
    owned(
        module_path!(),
        "a_tick_runs_at_each_due_opportunity_while_an_update_is_outstanding_and_a_press_arrives_once",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
            rig.backend.script.push_back(vec![left_press()]);
            // No answer is ever given: the first update stays outstanding for the whole run.
            quit_after_ticks(&mut rig, 5);
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(
                extract_updates(&ran.posts).len(),
                1,
                "no second update is posted while the first is outstanding"
            );
            assert!(ran.ticks.len() >= 5, "{}", ran.ticks.len());
            assert_eq!(
                ran.ticks[0].dt,
                Duration::ZERO,
                "the first tick after activation"
            );
            for tick in &ran.ticks[1..] {
                assert_eq!(
                    tick.dt,
                    Duration::from_nanos(16_666_666),
                    "one period apart"
                );
            }
            // Exactly once: the press reaches the link at one tick, and is never offered again once it was acknowledged.
            let presses = ran
                .ticks
                .iter()
                .flat_map(|tick| tick.input.transitions.iter())
                .filter(|transition| transition.edge == ck_input::Edge::Press)
                .count();
            assert_eq!(presses, 1);
            assert!(ran.ticks[1..].iter().all(|tick| {
                tick.input.transitions.is_empty()
                    && tick
                        .input
                        .actions
                        .iter()
                        .all(|action| !action.pressed && !action.released)
            }));
            assert!(
                ran.ticks[1..].iter().all(|tick| tick
                    .input
                    .actions
                    .iter()
                    .any(|a| a.name == "move_left" && a.held)),
                "the key stays held"
            );
            // The loop woke for the tick's opportunities, not only for the update's.
            assert!(ran.deadlines.iter().flatten().count() >= 4);
        },
    );
}

#[test]
fn opportunities_that_fell_due_together_coalesce_into_one_tick_with_a_capped_dt() {
    owned(
        module_path!(),
        "opportunities_that_fell_due_together_coalesce_into_one_tick_with_a_capped_dt",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
            lock(&rig.ui.update_answers).extend(std::iter::repeat_n(Answer::Returned, 4));
            // Five periods (83.3 ms) pass in one step between the first tick and the next pass.
            let fake_clock = clock.clone();
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            rig.backend.hook = Some(Box::new(move |call| {
                if call.index == 3 {
                    fake_clock.set(fake_clock.after(Duration::from_millis(84)));
                }
                if ui.ticks().len() >= 2 {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(
                ran.ticks.len(),
                2,
                "five periods, one tick: nothing is replayed"
            );
            assert_eq!(
                ran.ticks[1].dt,
                Duration::from_millis(20),
                "capped at [frame] max_delta_seconds"
            );
        },
    );
}

#[test]
fn a_busy_pass_with_nothing_due_runs_no_tick() {
    owned(
        module_path!(),
        "a_busy_pass_with_nothing_due_runs_no_tick",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
            // The clock never moves: many passes run (each pump has an event), and only opportunity zero is ever due.
            rig.backend.deadline_hook = None;
            for _ in 0..10 {
                rig.backend
                    .script
                    .push_back(vec![NativeEvent::NeedsDisplay]);
            }
            lock(&rig.ui.update_answers).extend(std::iter::repeat_n(Answer::Returned, 4));
            let pumps = Arc::clone(&rig.backend.pumps);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            rig.backend.hook = Some(Box::new(move |call| {
                if call.index >= 8 {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(ran.end, LoopEnd::Quit);
            assert!(lock(&pumps).len() >= 8, "the loop kept passing");
            assert_eq!(ran.ticks.len(), 1, "only the first opportunity fell due");
        },
    );
}

#[test]
fn no_tick_runs_while_unfocused_minimised_zero_sized_or_the_control_is_off() {
    owned(
        module_path!(),
        "no_tick_runs_while_unfocused_minimised_zero_sized_or_the_control_is_off",
        DEADLINE,
        || {
            for case in 0..4 {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                let control = configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
                match case {
                    0 => rig.backend.script.push_back(vec![NativeEvent::FocusLost]),
                    1 => rig.backend.script.push_back(vec![NativeEvent::Minimized]),
                    2 => rig.backend.script.push_back(vec![NativeEvent::SizeChanged {
                        points: (0, 0),
                        pixels: PixelSize {
                            width: 0,
                            height: 0,
                        },
                    }]),
                    _ => control.set(false),
                }
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.wait == Wait::UntilEvent {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ran = rig.run_collect_with_clock(&clock);
                assert_eq!(ran.end, LoopEnd::Quit, "case {case}");
                assert!(ran.ticks.is_empty(), "case {case}: {}", ran.ticks.len());
                assert!(extract_updates(&ran.posts).is_empty(), "case {case}");
            }
        },
    );
}

#[test]
fn ticks_continue_while_the_link_is_full_after_not_registered_and_with_a_close_outstanding() {
    owned(
        module_path!(),
        "ticks_continue_while_the_link_is_full_after_not_registered_and_with_a_close_outstanding",
        DEADLINE,
        || {
            // A link that refuses every update as full.
            {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
                rig.ui.full_update_posts.store(1_000, Ordering::SeqCst);
                quit_after_ticks(&mut rig, 4);
                let ran = rig.run_collect_with_clock(&clock);
                assert_eq!(ran.end, LoopEnd::Quit);
                assert!(
                    extract_updates(&ran.posts).is_empty(),
                    "every post was refused"
                );
                assert!(ran.ticks.len() >= 4);
            }
            // The epoch is suppressed by a NotRegistered answer: no further update, and the ticks go on, woken by their own deadline.
            {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
                lock(&rig.ui.update_answers).push_back(Answer::NotRegistered);
                quit_after_ticks(&mut rig, 4);
                let ran = rig.run_collect_with_clock(&clock);
                assert_eq!(ran.end, LoopEnd::Quit);
                assert_eq!(
                    extract_updates(&ran.posts).len(),
                    1,
                    "the suppressed epoch posts no more"
                );
                assert!(ran.ticks.len() >= 4);
            }
            // A close request is outstanding (nobody answers it).
            {
                let _scenario = scenario();
                let clock = FakeClock::new();
                let mut rig = Rig::new();
                configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.update_answers).extend(std::iter::repeat_n(Answer::Returned, 8));
                quit_after_ticks(&mut rig, 4);
                let ran = rig.run_collect_with_clock(&clock);
                assert_eq!(ran.end, LoopEnd::Quit);
                assert_eq!(
                    ran.posts
                        .iter()
                        .filter(|event| matches!(event, UiEvent::CloseRequested { .. }))
                        .count(),
                    1
                );
                assert!(ran.ticks.len() >= 4);
            }
        },
    );
}

/// The loop's measurement counts the UI cursor only: with ticks running and the UI refusing every post, `delivered` stays zero and
/// `missed` counts each UI opportunity that was replaced by the next, as it did before the ticks existed.
#[test]
fn ticks_do_not_change_what_the_loop_measures_of_the_ui_updates() {
    owned(
        module_path!(),
        "ticks_do_not_change_what_the_loop_measures_of_the_ui_updates",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let metrics = LoopMetrics::new(Arc::new(|| 1_000_000_000));
            metrics
                .request(MetricsWindow {
                    id: 1,
                    start: 1_000_000_000,
                    end: Some(2_000_000_000),
                })
                .expect("a first request");
            let mut rig = Rig::new();
            let updates_cfg = UpdateSettings::new(60, Duration::from_millis(20))
                .expect("valid settings")
                .with_metrics(metrics.clone());
            updates_cfg.control().set(true);
            rig.settings = LoopSettings::default().with_updates(updates_cfg);
            rig.backend.deadline_supported = true;
            let time = clock.clone();
            rig.backend.deadline_hook = Some(Box::new(move |deadline| time.set(deadline)));
            lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
            rig.ui.full_update_posts.store(1_000, Ordering::SeqCst);
            quit_after_ticks(&mut rig, 6);
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(ran.end, LoopEnd::Quit);
            assert!(ran.ticks.len() >= 6);
            let window = metrics.snapshot(1).expect("the window is kept").window;
            assert_eq!(
                window.stats.delivered, 0,
                "only accepted posts are deliveries: {window:?}"
            );
            // Every opportunity the refusing UI let pass was replaced by the next and counted once, by its own due time: the ticks add
            // nothing, and they take nothing away (the cursor that consumes them is not the UI cursor).
            let ticks = u64::try_from(ran.ticks.len()).expect("a small count");
            assert_eq!(window.stats.missed, ticks, "{window:?}");
        },
    );
}

#[test]
fn a_failing_input_ends_the_loop_with_its_message_before_any_update_is_posted() {
    owned(
        module_path!(),
        "a_failing_input_ends_the_loop_with_its_message_before_any_update_is_posted",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            configure_updates(&mut rig, &clock, 60, Duration::from_millis(20));
            rig.backend.script.push_back(vec![left_press()]);
            *lock(&rig.ui.input_failure) = Some("the map update failed".to_owned());
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(
                ran.end,
                LoopEnd::CallbackFailed {
                    message: "the map update failed".to_owned()
                }
            );
            assert!(ran.ticks.is_empty(), "the failing tick consumed nothing");
            assert!(extract_updates(&ran.posts).is_empty());
        },
    );
}

#[test]
fn a_size_change_reaches_the_presenter_whole_and_once_before_the_tick_that_carries_it() {
    owned(
        module_path!(),
        "a_size_change_reaches_the_presenter_whole_and_once_before_the_tick_that_carries_it",
        DEADLINE,
        || {
            let _scenario = scenario();
            let clock = FakeClock::new();
            let mut rig = Rig::new();
            let update_settings =
                UpdateSettings::new(60, Duration::from_millis(20)).expect("valid settings");
            update_settings.control().set(true);
            rig.settings = LoopSettings::default().with_updates(update_settings);
            rig.backend.deadline_supported = true;
            let fake_time = clock.clone();
            rig.backend.deadline_hook = Some(Box::new(move |deadline| fake_time.set(deadline)));
            lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
            // 640x400 points at 2 pixels a point open; the first pass drains a resize to 900x600 points (1800x1200 pixels).
            rig.backend.script.push_back(vec![NativeEvent::SizeChanged {
                points: (900, 600),
                pixels: PixelSize {
                    width: 1800,
                    height: 1200,
                },
            }]);
            let ui = Arc::clone(&rig.ui);
            let quit = Arc::clone(&rig.quit);
            let notifier = Arc::clone(&rig.notifier);
            rig.backend.hook = Some(Box::new(move |_| {
                if ui.ticks().len() >= 2 {
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                }
            }));
            let ran = rig.run_collect_with_clock(&clock);
            assert_eq!(ran.end, LoopEnd::Quit);
            let sizes: Vec<_> = ran
                .log
                .iter()
                .enumerate()
                .filter(|(_, line)| line.starts_with("presenter.set_size"))
                .collect();
            assert_eq!(sizes.len(), 1, "once: {sizes:?}");
            assert_eq!(
                sizes[0].1,
                "presenter.set_size 900x600 points 1800x1200 pixels"
            );
            let first_tick = ran
                .log
                .iter()
                .position(|line| line.starts_with("ui.input"))
                .expect("a tick was consumed");
            assert!(
                sizes[0].0 < first_tick,
                "the presenter was told before the first tick: {:?}",
                ran.log
            );
            // The tick, and every later one, carries the new size: none is built from the old.
            assert!(
                ran.ticks
                    .iter()
                    .all(|tick| tick.metrics.points == (900, 600)
                        && tick.metrics.pixels
                            == PixelSize {
                                width: 1800,
                                height: 1200,
                            })
            );
        },
    );
}

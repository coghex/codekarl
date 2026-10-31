//! Bounded work per pass, quit observed versus teardown entered, the check-to-wait race, wakes of a
//! sleeping loop and backlogs.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use ck_window::{Answer, LoopEnd, LoopSettings, NativeEvent, PresenterReport, UiEvent, Wait};

use crate::fakes::{DEADLINE, Rig, at, lock, owned, scenario};

/// Stale answers are harmless: a decline of a request that is not outstanding.
fn stale_answer() -> (UiEvent, Answer) {
    (
        UiEvent::CloseRequested {
            source: ck_window::CloseSource::Key,
        },
        Answer::Decline,
    )
}

#[test]
fn a_draining_backend_delivers_every_event_in_one_pump_and_the_loop_stays_within_its_caps() {
    owned(
        module_path!(),
        "a_draining_backend_delivers_every_event_in_one_pump_and_the_loop_stays_within_its_caps",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, per_pass) = {
                let mut rig = Rig::new();
                rig.settings = LoopSettings::new(
                    8,
                    16,
                    std::time::Duration::from_secs(2),
                    std::time::Duration::from_millis(100),
                )
                .expect("non-zero");
                rig.backend.draining = true;
                rig.backend
                    .script
                    .push_back(vec![NativeEvent::NeedsDisplay; 500]);
                for _ in 0..100 {
                    lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                    let (event, answer) = stale_answer();
                    lock(&rig.ui.answers).push_back(ck_window::UiAnswer { event, answer });
                }
                // At each pump, record how many reports and answers the loop had taken so far; the last pass ends
                // the run once everything was consumed.
                let samples = Arc::new(std::sync::Mutex::new(Vec::new()));
                let taken = Arc::clone(&rig.presenter.taken);
                let ui = Arc::clone(&rig.ui);
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                let seen = Arc::clone(&samples);
                rig.backend.hook = Some(Box::new(move |_| {
                    let reports = taken.load(Ordering::SeqCst);
                    let answers = ui.answers_taken.load(Ordering::SeqCst);
                    lock(&seen).push((reports, answers));
                    if reports == 100 && answers == 100 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ran = rig.run_collect();
                let per_pass = lock(&samples).clone();
                (ran, per_pass)
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(
                ran.delivered[0], 500,
                "a draining pump delivered all 500 events in one call"
            );
            // Between two pumps the loop took at most the cap (16) of reports and of answers.
            for pair in per_pass.windows(2) {
                assert!(
                    pair[1].0 - pair[0].0 <= 16,
                    "reports in one pass: {per_pass:?}"
                );
                assert!(
                    pair[1].1 - pair[0].1 <= 16,
                    "answers in one pass: {per_pass:?}"
                );
            }
            assert_eq!(
                *per_pass.last().expect("passes"),
                (100, 100),
                "every item was eventually taken"
            );
        },
    );
}

#[test]
fn a_capped_backend_stops_after_the_cap_and_the_rest_is_drained_on_later_passes() {
    owned(
        module_path!(),
        "a_capped_backend_stops_after_the_cap_and_the_rest_is_drained_on_later_passes",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.settings = LoopSettings::new(
                    8,
                    16,
                    std::time::Duration::from_secs(2),
                    std::time::Duration::from_millis(100),
                )
                .expect("non-zero");
                rig.backend
                    .script
                    .push_back(vec![NativeEvent::NeedsDisplay; 50]);
                let delivered = Arc::clone(&rig.backend.delivered);
                let quit = Arc::clone(&rig.quit);
                let notifier = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |_| {
                    if lock(&delivered).iter().sum::<usize>() >= 50 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert!(
                ran.delivered.iter().all(|count| *count <= 8),
                "{:?}",
                ran.delivered
            );
            assert_eq!(ran.delivered.iter().sum::<usize>(), 50);
            assert!(
                ran.delivered.len() >= 7,
                "the cap forced several pumps: {:?}",
                ran.delivered
            );
        },
    );
}

#[test]
fn a_quit_set_while_the_pump_holds_control_is_seen_on_the_first_pass_after_it_returns() {
    owned(
        module_path!(),
        "a_quit_set_while_the_pump_holds_control_is_seen_on_the_first_pass_after_it_returns",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, taken, answers) = {
                let mut rig = Rig::new();
                for _ in 0..100 {
                    lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                    let (event, answer) = stale_answer();
                    lock(&rig.ui.answers).push_back(ck_window::UiAnswer { event, answer });
                }
                let quit = Arc::clone(&rig.quit);
                let writer = rig.writer.clone();
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 1 {
                        // A request arrives, and the quit with it, while the pump holds control.
                        writer.set_clear([0.4; 3]).expect("finite");
                        quit.store(true, Ordering::SeqCst);
                    }
                }));
                let taken = Arc::clone(&rig.presenter.taken);
                let ui = Arc::clone(&rig.ui);
                let ran = rig.run_collect();
                (
                    ran,
                    taken.load(Ordering::SeqCst),
                    ui.answers_taken.load(Ordering::SeqCst),
                )
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            // No pass read anything after the quit; teardown then reads all 100 queued reports of the
            // finished presenter (the per-pass cap does not apply to it) and no UI answers.
            assert_eq!(
                (taken, answers),
                (100, 0),
                "only teardown took reports after the quit"
            );
            assert!(
                !ran.log
                    .iter()
                    .any(|line| line.starts_with("presenter.set_clear")),
                "the request slot was not even read after the quit: {:#?}",
                ran.log
            );
            // The pump returned first; only then did teardown start (the fake fails the test if a teardown call
            // happens inside pump).
            assert!(at(&ran.log, "backend.pump exit #1") < at(&ran.log, "presenter.request_stop"));
            assert!(at(&ran.log, "presenter.poll_end") < at(&ran.log, "backend.destroy_window"));
            assert!(at(&ran.log, "backend.destroy_window") < at(&ran.log, "backend.terminate"));
        },
    );
}

#[test]
fn a_wake_between_the_pending_check_and_the_wait_does_not_let_the_loop_sleep() {
    owned(
        module_path!(),
        "a_wake_between_the_pending_check_and_the_wait_does_not_let_the_loop_sleep",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, timed_out) = {
                let mut rig = Rig::new();
                let notifier = Arc::clone(&rig.notifier);
                let quit = Arc::clone(&rig.quit);
                let wake_after = Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |call| match call.index {
                    // The flag the `open` write set is consumed here, so the next pass really sees it clear.
                    1 => assert!(notifier.take_pending()),
                    // Pass 2 decided to sleep (the flag was false); a wake lands now, after that check and
                    // before the wait. The wake is sticky, so the pump returns at once.
                    2 => {
                        assert_eq!(call.wait, Wait::UntilEvent);
                        notifier.wake();
                    }
                    _ => {
                        quit.store(true, Ordering::SeqCst);
                        wake_after.wake();
                    }
                }));
                let timed_out = Arc::clone(&rig.backend.timed_out);
                let ran = rig.run_collect();
                (ran, timed_out.load(Ordering::SeqCst))
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert!(!timed_out, "the pump slept with a wake queued");
            assert!(
                !ran.log.iter().any(|line| line == "backend.pump sleeping"),
                "the pump never went to sleep: {:#?}",
                ran.log
            );
        },
    );
}

#[test]
fn a_sleeping_loop_is_woken_by_a_request_a_ui_answer_and_a_presenter_report_alone() {
    owned(
        module_path!(),
        "a_sleeping_loop_is_woken_by_a_request_a_ui_answer_and_a_presenter_report_alone",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::new();
                let native = Arc::clone(&rig.backend.native);
                let writer = rig.writer.clone();
                let ui = Arc::clone(&rig.ui);
                let reports = Arc::clone(&rig.presenter.reports);
                let notifier = Arc::clone(&rig.notifier);
                let driver = thread::spawn(move || {
                    // 1. a request wakes the sleeping loop.
                    native.wait_sleeping(1);
                    writer.set_clear([0.9, 0.8, 0.7]).expect("finite");
                    // 2. a UI answer, through the link's result wake.
                    native.wait_sleeping(2);
                    ui.answer(
                        UiEvent::CloseRequested {
                            source: ck_window::CloseSource::Key,
                        },
                        Answer::Decline,
                    );
                    // 3. a presenter report, through the presenter's report wake.
                    native.wait_sleeping(3);
                    lock(&reports).push_back(PresenterReport::FirstFrameQueued);
                    notifier.wake();
                    // 4. and finally a close request ends the loop.
                    native.wait_sleeping(4);
                    writer.close();
                });
                let ran = rig.run_collect();
                driver.join().expect("the driver finished");
                ran
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(
                ran.log
                    .iter()
                    .any(|line| line == "presenter.set_clear [0.9, 0.8, 0.7]"),
                "the request was applied: {:#?}",
                ran.log
            );
            assert_eq!(
                ran.posts,
                [UiEvent::Opened],
                "the report was handled and Opened sent"
            );
            assert!(
                ran.waits
                    .iter()
                    .filter(|wait| **wait == Wait::UntilEvent)
                    .count()
                    >= 4
            );
        },
    );
}

#[test]
fn a_backlog_of_presenter_reports_larger_than_the_cap_is_drained_with_a_single_wake() {
    owned(
        module_path!(),
        "a_backlog_of_presenter_reports_larger_than_the_cap_is_drained_with_a_single_wake",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                for _ in 0..40 {
                    lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                }
                let notifier = Arc::clone(&rig.notifier);
                let quit = Arc::clone(&rig.quit);
                let taken = Arc::clone(&rig.presenter.taken);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 1 {
                        // Only the `open` write's wake exists, and it is consumed here: no further wake comes.
                        assert!(notifier.take_pending());
                    }
                    if call.wait == Wait::UntilEvent {
                        // The backlog (16 + 16 + 8) was drained on three polling passes without a wake.
                        assert_eq!(taken.load(Ordering::SeqCst), 40);
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(
                ran.waits,
                [Wait::Poll, Wait::Poll, Wait::Poll, Wait::UntilEvent],
                "the loop polled until the backlog was observed exhausted, and only then slept"
            );
        },
    );
}

#[test]
fn a_backlog_of_ui_answers_larger_than_the_cap_is_drained_with_a_single_wake() {
    owned(
        module_path!(),
        "a_backlog_of_ui_answers_larger_than_the_cap_is_drained_with_a_single_wake",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, taken) = {
                let mut rig = Rig::new();
                for _ in 0..40 {
                    let (event, answer) = stale_answer();
                    lock(&rig.ui.answers).push_back(ck_window::UiAnswer { event, answer });
                }
                let notifier = Arc::clone(&rig.notifier);
                let quit = Arc::clone(&rig.quit);
                let ui = Arc::clone(&rig.ui);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 1 {
                        assert!(notifier.take_pending());
                    }
                    if call.wait == Wait::UntilEvent {
                        assert_eq!(ui.answers_taken.load(Ordering::SeqCst), 40);
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                let ui = Arc::clone(&rig.ui);
                let ran = rig.run_collect();
                (ran, ui.answers_taken.load(Ordering::SeqCst))
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(taken, 40);
            assert_eq!(
                ran.waits,
                [Wait::Poll, Wait::Poll, Wait::Poll, Wait::UntilEvent]
            );
        },
    );
}

#[test]
fn the_pixel_size_and_a_redraw_request_reach_the_presenter_once_per_change() {
    owned(
        module_path!(),
        "the_pixel_size_and_a_redraw_request_reach_the_presenter_once_per_change",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(vec![
                    NativeEvent::PixelSizeChanged {
                        width: 1,
                        height: 2,
                    },
                    NativeEvent::PixelSizeChanged {
                        width: 1600,
                        height: 1200,
                    },
                    NativeEvent::NeedsDisplay,
                    NativeEvent::NeedsDisplay,
                ]);
                rig.quit_at_pump(2);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            let sizes: Vec<_> = ran
                .log
                .iter()
                .filter(|line| line.starts_with("presenter.set_pixel_size"))
                .collect();
            assert_eq!(
                sizes,
                ["presenter.set_pixel_size 1600x1200"],
                "coalesced to the latest"
            );
            let redraws = ran
                .log
                .iter()
                .filter(|line| *line == "presenter.request_redraw")
                .count();
            assert_eq!(redraws, 1, "two NeedsDisplay events are one redraw request");
        },
    );
}

#[test]
fn an_event_refused_with_full_does_not_keep_the_loop_polling_and_a_result_wake_ends_the_wait() {
    owned(
        module_path!(),
        "an_event_refused_with_full_does_not_keep_the_loop_polling_and_a_result_wake_ends_the_wait",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, pumps_while_blocked) = {
                let rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.ui.blocked.store(true, Ordering::SeqCst);
                let native = Arc::clone(&rig.backend.native);
                let pumps = Arc::clone(&rig.backend.pumps);
                let ui = Arc::clone(&rig.ui);
                let writer = rig.writer.clone();
                let driver = thread::spawn(move || {
                    // The loop offers Opened, is refused, and goes to sleep instead of spinning.
                    native.wait_sleeping(1);
                    thread::sleep(std::time::Duration::from_millis(150));
                    let blocked_pumps = lock(&pumps).len();
                    assert!(ui.posted().is_empty(), "nothing could be posted yet");
                    // The UI VM produces a result: its wake ends the wait, and the event is posted once.
                    ui.release_capacity();
                    native.wait_sleeping(2);
                    writer.close();
                    blocked_pumps
                });
                let ran = rig.run_collect();
                (ran, driver.join().expect("the driver finished"))
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(
                ran.posts,
                [UiEvent::Opened],
                "posted once, after the capacity came back"
            );
            assert!(
                pumps_while_blocked <= 4,
                "a bounded number of passes, not a spin: {pumps_while_blocked}"
            );
        },
    );
}

#[test]
fn a_quit_wakes_a_loop_that_is_waiting_for_a_full_link() {
    owned(
        module_path!(),
        "a_quit_wakes_a_loop_that_is_waiting_for_a_full_link",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.ui.blocked.store(true, Ordering::SeqCst);
                let native = Arc::clone(&rig.backend.native);
                let (quit, notifier) = (Arc::clone(&rig.quit), Arc::clone(&rig.notifier));
                let driver = thread::spawn(move || {
                    native.wait_sleeping(1);
                    // The signal thread: set the flag, then wake.
                    quit.store(true, Ordering::SeqCst);
                    notifier.wake();
                });
                let ran = rig.run_collect();
                driver.join().expect("the driver finished");
                ran
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert!(ran.posts.is_empty(), "the link never had room");
        },
    );
}

#[test]
fn several_events_blocked_by_full_are_posted_once_each_when_capacity_returns() {
    owned(
        module_path!(),
        "several_events_blocked_by_full_are_posted_once_each_when_capacity_returns",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                rig.ui.blocked.store(true, Ordering::SeqCst);
                lock(&rig.ui.close_answers).push_back(Answer::Accept);
                let (native, ui) = (Arc::clone(&rig.backend.native), Arc::clone(&rig.ui));
                let driver = thread::spawn(move || {
                    native.wait_sleeping(1);
                    ui.release_capacity();
                });
                let ran = rig.run_collect();
                driver.join().expect("the driver finished");
                ran
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(
                ran.posts,
                [
                    UiEvent::Opened,
                    UiEvent::CloseRequested {
                        source: ck_window::CloseSource::Button
                    }
                ]
            );
        },
    );
}

#[test]
fn a_pending_wake_makes_the_next_pass_poll() {
    owned(
        module_path!(),
        "a_pending_wake_makes_the_next_pass_poll",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                let (notifier, quit) = (Arc::clone(&rig.notifier), Arc::clone(&rig.quit));
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 1 {
                        // The flag the `open` write set is consumed, and a fresh wake (say, from the signal
                        // thread) lands during pass 1: it is still pending when pass 2 decides how to wait.
                        assert!(notifier.take_pending());
                        notifier.wake();
                    } else {
                        assert_eq!(
                            call.wait,
                            Wait::Poll,
                            "a pending wake must not be slept through"
                        );
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
        },
    );
}

//! The close protocol and the bounded posting of UI events.

use std::sync::atomic::Ordering;

use ck_window::{Answer, CloseSource, LoopEnd, NativeEvent, PresenterReport, UiEvent};

use crate::fakes::{DEADLINE, Rig, at, lock, owned, scenario};

fn escape_pair() -> [NativeEvent; 2] {
    [
        NativeEvent::Key {
            key: ck_input::Key::Escape,
            down: true,
            repeat: false,
        },
        NativeEvent::Key {
            key: ck_input::Key::Escape,
            down: false,
            repeat: false,
        },
    ]
}

fn press(source: CloseSource) -> UiEvent {
    UiEvent::CloseRequested { source }
}

#[test]
fn accept_closes_the_window_and_opened_follows_the_first_frame() {
    owned(
        module_path!(),
        "accept_closes_the_window_and_opened_follows_the_first_frame",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Accept);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(ran.posts, [UiEvent::Opened, press(CloseSource::Button)]);
            // The window closed before the loop returned: teardown ran.
            assert!(
                ran.log
                    .iter()
                    .any(|line| line.starts_with("backend.terminate")),
                "{:#?}",
                ran.log
            );
        },
    );
}

#[test]
fn decline_keeps_the_window_open_and_a_new_press_is_a_new_request() {
    owned(
        module_path!(),
        "decline_keeps_the_window_open_and_a_new_press_is_a_new_request",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                rig.backend.script.push_back(Vec::new());
                rig.backend.script.push_back(escape_pair().into());
                lock(&rig.ui.close_answers).extend([Answer::Decline, Answer::Accept]);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(
                ran.posts,
                [press(CloseSource::Button), press(CloseSource::Key)],
                "the declined request was answered, so the next press is a new request"
            );
            // The window stayed open between the two: nothing was destroyed before the accepted close.
            let destroyed = at(&ran.log, "backend.destroy_window");
            let second = ran
                .log
                .iter()
                .rposition(|line| line == "ui.post CloseRequested { source: Key }")
                .expect("posted");
            assert!(destroyed > second, "{:#?}", ran.log);
        },
    );
}

#[test]
fn not_registered_means_accept() {
    owned(
        module_path!(),
        "not_registered_means_accept",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(escape_pair().into());
                lock(&rig.ui.close_answers).push_back(Answer::NotRegistered);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(ran.posts, [press(CloseSource::Key)]);
        },
    );
}

#[test]
fn a_failed_callback_ends_the_loop_with_its_message_and_the_window_is_still_torn_down() {
    owned(
        module_path!(),
        "a_failed_callback_ends_the_loop_with_its_message_and_the_window_is_still_torn_down",
        DEADLINE,
        || {
            let _scenario = scenario();
            let message =
                "ui callback on_close_request failed: boom\nstack traceback:\n\t[C]: in ?";
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Failed {
                    message: message.to_owned(),
                });
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::CallbackFailed {
                    message: message.to_owned()
                }
            );
            assert!(
                ran.log
                    .iter()
                    .any(|line| line.starts_with("backend.destroy_window"))
            );
        },
    );
}

#[test]
fn a_failed_answer_to_opened_also_ends_the_loop() {
    owned(
        module_path!(),
        "a_failed_answer_to_opened_also_ends_the_loop",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.ui.answer(
                    UiEvent::Opened,
                    Answer::Failed {
                        message: "on_opened failed".to_owned(),
                    },
                );
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::CallbackFailed {
                    message: "on_opened failed".to_owned()
                }
            );
        },
    );
}

#[test]
fn a_burst_of_presses_leaves_one_outstanding_until_it_is_answered() {
    owned(
        module_path!(),
        "a_burst_of_presses_leaves_one_outstanding_until_it_is_answered",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(vec![
                    NativeEvent::CloseButton,
                    NativeEvent::Key {
                        key: ck_input::Key::Escape,
                        down: true,
                        repeat: false,
                    },
                    NativeEvent::Key {
                        key: ck_input::Key::Escape,
                        down: false,
                        repeat: false,
                    },
                    NativeEvent::Key {
                        key: ck_input::Key::Escape,
                        down: true,
                        repeat: false,
                    },
                    NativeEvent::Key {
                        key: ck_input::Key::Escape,
                        down: false,
                        repeat: false,
                    },
                    NativeEvent::CloseButton,
                ]);
                // Key repeat while the request is still outstanding, and no answer waiting for it yet.
                rig.backend.script.push_back(escape_pair().into());
                // The decline arrives only at pump 3, after the second batch (pump 2) was pumped with the request still
                // outstanding: that batch is absorbed. The quit comes at pump 5.
                let ui = std::sync::Arc::clone(&rig.ui);
                let quit = std::sync::Arc::clone(&rig.quit);
                let notifier = std::sync::Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 3 {
                        ui.answer(press(CloseSource::Button), Answer::Decline);
                    }
                    if call.index == 5 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(
                ran.posts,
                [press(CloseSource::Button)],
                "one request, from the first press"
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// A close evaluated after the UI's answers (CK-214)
// ---------------------------------------------------------------------------------------------

fn escape_down() -> NativeEvent {
    NativeEvent::Key {
        key: ck_input::Key::Escape,
        down: true,
        repeat: false,
    }
}

/// Pass 1 pumps the first close and posts its request; the link answers `Decline` when it is posted, so that answer is **waiting**
/// when pass 2 pumps `second`. The run quits at pump 3, so a request that `second` makes must be posted in pass 2 itself.
fn declined_then(
    second: Vec<NativeEvent>,
    first: NativeEvent,
) -> (Vec<UiEvent>, Vec<String>, Vec<String>) {
    let scenario = scenario();
    let ran = {
        let mut rig = Rig::new();
        rig.backend.script.push_back(vec![first]);
        rig.backend.script.push_back(second);
        lock(&rig.ui.close_answers).push_back(Answer::Decline);
        rig.quit_at_pump(3);
        rig.run_collect()
    };
    assert_eq!(ran.end, LoopEnd::Quit);
    let records = scenario.records();
    (ran.posts, ran.log, records)
}

#[test]
fn a_key_press_pumped_while_a_decline_waits_is_a_new_request_in_that_pass() {
    owned(
        module_path!(),
        "a_key_press_pumped_while_a_decline_waits_is_a_new_request_in_that_pass",
        DEADLINE,
        || {
            let (posts, log, records) = declined_then(
                vec![
                    escape_down(),
                    NativeEvent::Key {
                        key: ck_input::Key::Escape,
                        down: false,
                        repeat: false,
                    },
                ],
                NativeEvent::CloseButton,
            );
            assert_eq!(
                posts,
                [press(CloseSource::Button), press(CloseSource::Key)],
                "the second press was not absorbed by the request the waiting Decline had already retired"
            );
            // The second request was posted in pass 2: after the second pump entered and before the third.
            let second_pump = at(&log, "backend.pump enter #2");
            let second_post = at(&log, "ui.post CloseRequested { source: Key }");
            assert!(second_pump < second_post, "{log:#?}");
            assert!(
                !log.iter()
                    .any(|line| line.starts_with("backend.pump enter #3"))
                    || at(&log, "backend.pump enter #3") > second_post,
                "{log:#?}"
            );
            assert!(records.is_empty(), "nothing was absorbed: {records:?}");
        },
    );
}

#[test]
fn a_close_button_pumped_while_a_decline_waits_is_a_new_request_in_that_pass() {
    owned(
        module_path!(),
        "a_close_button_pumped_while_a_decline_waits_is_a_new_request_in_that_pass",
        DEADLINE,
        || {
            let (posts, log, records) =
                declined_then(vec![NativeEvent::CloseButton], NativeEvent::CloseButton);
            assert_eq!(
                posts,
                [press(CloseSource::Button), press(CloseSource::Button)],
                "button closes follow the same rule"
            );
            let second_pump = at(&log, "backend.pump enter #2");
            let second_post = log
                .iter()
                .rposition(|line| line == "ui.post CloseRequested { source: Button }")
                .expect("posted");
            assert!(second_pump < second_post, "{log:#?}");
            assert!(records.is_empty(), "nothing was absorbed: {records:?}");
        },
    );
}

#[test]
fn with_a_decline_waiting_a_button_and_a_press_in_one_pump_make_one_request_and_absorb_the_other() {
    owned(
        module_path!(),
        "with_a_decline_waiting_a_button_and_a_press_in_one_pump_make_one_request_and_absorb_the_other",
        DEADLINE,
        || {
            let (posts, _log, records) = declined_then(
                vec![escape_down(), NativeEvent::CloseButton],
                NativeEvent::CloseButton,
            );
            assert_eq!(
                posts,
                [press(CloseSource::Button), press(CloseSource::Button)],
                "the button is evaluated first (the order the evaluation had before the move), so the press is the absorbed one"
            );
            assert_eq!(
                records,
                ["DEBUG window: 1 close request(s) absorbed while one was outstanding"]
            );
        },
    );
}

#[test]
fn a_press_while_the_request_is_outstanding_and_no_answer_waits_is_still_absorbed() {
    owned(
        module_path!(),
        "a_press_while_the_request_is_outstanding_and_no_answer_waits_is_still_absorbed",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                rig.backend
                    .script
                    .push_back(vec![escape_down(), NativeEvent::CloseButton]);
                // No scripted answer: the request stays outstanding through pass 2.
                rig.quit_at_pump(3);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(ran.posts, [press(CloseSource::Button)]);
            assert_eq!(
                scenario.records(),
                ["DEBUG window: 2 close request(s) absorbed while one was outstanding"]
            );
        },
    );
}

#[test]
fn a_close_from_the_slot_closes_without_asking_the_callback() {
    owned(
        module_path!(),
        "a_close_from_the_slot_closes_without_asking_the_callback",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                let writer = rig.writer.clone();
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 2 {
                        writer.close();
                    }
                }));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(
                !ran.posts
                    .iter()
                    .any(|event| matches!(event, UiEvent::CloseRequested { .. })),
                "{:?}",
                ran.posts
            );
        },
    );
}

#[test]
fn opened_is_sent_once_and_only_after_the_first_frame_is_reported() {
    owned(
        module_path!(),
        "opened_is_sent_once_and_only_after_the_first_frame_is_reported",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                let reports = std::sync::Arc::clone(&rig.presenter.reports);
                let quit = std::sync::Arc::clone(&rig.quit);
                let notifier = std::sync::Arc::clone(&rig.notifier);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 3 {
                        let mut reports = lock(&reports);
                        reports.push_back(PresenterReport::FirstFrameQueued);
                        reports.push_back(PresenterReport::FirstFrameQueued);
                        drop(reports);
                        // The presenter's report wake.
                        notifier.wake();
                    }
                    if call.index == 5 {
                        quit.store(true, Ordering::SeqCst);
                        notifier.wake();
                    }
                }));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(ran.posts, [UiEvent::Opened], "sent once");
            assert!(
                at(&ran.log, "ui.post Opened") > at(&ran.log, "backend.pump enter #3"),
                "not before the report: {:#?}",
                ran.log
            );
        },
    );
}

#[test]
fn a_full_link_keeps_at_most_one_close_request_and_one_opened_and_posts_them_later() {
    owned(
        module_path!(),
        "a_full_link_keeps_at_most_one_close_request_and_one_opened_and_posts_them_later",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, attempts) = {
                let mut rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                rig.backend.script.push_back(escape_pair().into());
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                rig.ui.blocked.store(true, Ordering::SeqCst);
                lock(&rig.ui.close_answers).push_back(Answer::Accept);
                let ui = std::sync::Arc::clone(&rig.ui);
                // The UI VM frees a slot (its result wake) only after all three presses have come and gone.
                let releasing = std::sync::Arc::clone(&ui);
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 4 {
                        releasing.release_capacity();
                    }
                }));
                let ran = rig.run_collect();
                (ran, ui.post_attempts.load(Ordering::SeqCst))
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(
                ran.posts,
                [UiEvent::Opened, press(CloseSource::Button)],
                "one of each, however many presses there were"
            );
            assert!(
                attempts >= 6,
                "the events were retried while the link was full: {attempts}"
            );
        },
    );
}

#[test]
fn a_disconnected_link_ends_the_loop_with_a_failure() {
    owned(
        module_path!(),
        "a_disconnected_link_ends_the_loop_with_a_failure",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::new();
                rig.ui.disconnected.store(true, Ordering::SeqCst);
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::Failed {
                    message: "the ui VM ended".to_owned()
                }
            );
            assert!(
                ran.log
                    .iter()
                    .any(|line| line.starts_with("backend.terminate")),
                "torn down"
            );

            let ran = {
                let rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.ui.disconnect_on_post.store(true, Ordering::SeqCst);
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::Failed {
                    message: "the ui VM ended".to_owned()
                }
            );
        },
    );
}

//! The loop's records: category `window`, `log_within` only, exactly once each.

use ck_window::{Answer, LoopEnd, NativeEvent, PresenterReport};

use crate::fakes::{DEADLINE, Rig, lock, owned, scenario};

#[test]
fn window_opened_and_window_closed_appear_once_each_and_in_order_on_a_normal_close() {
    owned(
        module_path!(),
        "window_opened_and_window_closed_appear_once_each_and_in_order_on_a_normal_close",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::FirstFrameQueued);
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Accept);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(
                scenario.records(),
                ["INFO  window: window opened", "INFO  window: window closed"]
            );
        },
    );
}

#[test]
fn a_window_that_never_reported_a_frame_logs_neither_record() {
    owned(
        module_path!(),
        "a_window_that_never_reported_a_frame_logs_neither_record",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(scenario.records().is_empty(), "{:?}", scenario.records());
        },
    );
}

#[test]
fn absorbed_presses_are_counted_in_one_debug_record_per_pass() {
    owned(
        module_path!(),
        "absorbed_presses_are_counted_in_one_debug_record_per_pass",
        DEADLINE,
        || {
            let scenario = scenario();
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
                rig.backend.script.push_back(vec![
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
                ]);
                rig.quit_at_pump(3);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert_eq!(
                scenario.records(),
                [
                    "DEBUG window: 3 close request(s) absorbed while one was outstanding",
                    "DEBUG window: 1 close request(s) absorbed while one was outstanding",
                ]
            );
        },
    );
}

const SOURCES: [(&str, &str); 4] = [
    ("main_loop.rs", include_str!("../../src/main_loop.rs")),
    ("requests.rs", include_str!("../../src/requests.rs")),
    ("notifier.rs", include_str!("../../src/notifier.rs")),
    ("lib.rs", include_str!("../../src/lib.rs")),
];

#[test]
fn no_record_uses_the_waiting_log_and_only_one_place_emits() {
    for (name, source) in SOURCES {
        for forbidden in [
            "ck_log::error!",
            "ck_log::warn!",
            "ck_log::info!",
            "ck_log::debug!",
            "ck_log::trace!",
            "ck_log::log_global",
            ".log(",
            "ck_log::try_log",
        ] {
            assert!(
                !source.contains(forbidden),
                "src/{name} contains the waiting or unbounded call `{forbidden}`"
            );
        }
    }
    let main_loop = SOURCES[0].1;
    assert_eq!(
        main_loop.matches("ck_log::log_within!").count(),
        1,
        "one place emits the records"
    );
    for (name, source) in SOURCES {
        if name != "main_loop.rs" {
            assert!(
                !source.contains("ck_log::log_within!"),
                "src/{name} emits a record"
            );
        }
    }
}

#[test]
fn the_crate_has_no_unsafe_code_and_no_drop_that_releases_backend_objects() {
    const ALL: [(&str, &str); 7] = [
        ("backend.rs", include_str!("../../src/backend.rs")),
        ("desc.rs", include_str!("../../src/desc.rs")),
        ("link.rs", include_str!("../../src/link.rs")),
        ("main_loop.rs", include_str!("../../src/main_loop.rs")),
        ("notifier.rs", include_str!("../../src/notifier.rs")),
        ("requests.rs", include_str!("../../src/requests.rs")),
        ("settings.rs", include_str!("../../src/settings.rs")),
    ];
    for (name, source) in ALL {
        assert!(
            !source.contains("unsafe "),
            "src/{name} contains unsafe code"
        );
        if name != "notifier.rs" {
            assert!(!source.contains("impl Drop"), "src/{name} has a Drop impl");
        }
    }
    // The one Drop (the notifier's in-flight guard) releases no backend or presenter object.
    assert_eq!(ALL[4].1.matches("impl Drop").count(), 1);
    let main_loop = ALL[3].1;
    assert!(main_loop.contains("ManuallyDrop::new(backend)"));
    assert!(main_loop.contains("ManuallyDrop::new(presenter)"));
}

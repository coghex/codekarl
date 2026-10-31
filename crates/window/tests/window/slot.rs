//! The request slot: last-wins state, one-shot flags, the end of the loop, wakes, and validation.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{
    InvalidColour, LoopSettings, MAX_DIMENSION, NativeWake, Notifier, OpenError, RequestChanges,
    Requests, WindowDesc,
};

use crate::fakes::{BOUND, SHORT, owned};

fn desc() -> WindowDesc {
    WindowDesc {
        title: "slot".to_owned(),
        width: 800,
        height: 600,
        clear: [0.0, 0.5, 1.0],
    }
}

fn fixture() -> (
    Requests,
    ck_window::RequestWriter,
    ck_window::RequestReader,
    Arc<Notifier>,
) {
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    (requests, writer, reader, notifier)
}

#[test]
fn a_thousand_set_clear_calls_between_two_takes_give_one_change_with_the_last_colour() {
    let (_, writer, reader, _) = fixture();
    for step in 0..1000_u16 {
        writer
            .set_clear([f32::from(step) / 1000.0, 0.0, 0.0])
            .expect("finite");
    }
    assert_eq!(
        reader.take(),
        RequestChanges {
            open: None,
            clear: Some([0.999, 0.0, 0.0]),
            close: false,
        }
    );
    assert_eq!(
        reader.take(),
        RequestChanges::default(),
        "nothing changed since"
    );
}

#[test]
fn open_and_close_are_one_shot() {
    let (_, writer, reader, _) = fixture();
    writer.open(desc()).expect("the first open");
    assert_eq!(writer.open(desc()), Err(OpenError::AlreadyOpened));
    assert_eq!(reader.take().open, Some(desc()));
    assert_eq!(
        writer.open(desc()),
        Err(OpenError::AlreadyOpened),
        "also after the first was taken"
    );
    writer.close();
    writer.close();
    assert!(reader.take().close);
    writer.close();
    assert!(
        !reader.take().close,
        "a close after a taken close changes nothing"
    );
}

#[test]
fn writes_after_end_are_invisible_and_counted() {
    let (requests, writer, reader, _) = fixture();
    reader.end();
    assert_eq!(requests.discarded(), 0);
    writer.open(desc()).expect("returns at once");
    writer.set_clear([1.0, 1.0, 1.0]).expect("returns at once");
    writer.close();
    assert_eq!(requests.discarded(), 3);
    assert_eq!(reader.take(), RequestChanges::default(), "nothing visible");
    // An invalid write is refused, not discarded.
    assert!(writer.set_clear([f32::NAN, 0.0, 0.0]).is_err());
    assert_eq!(requests.discarded(), 3);
}

#[test]
fn open_after_end_is_already_opened_when_there_was_an_open_and_discarded_when_there_was_not() {
    // The one-shot check comes before the ended check: a script that opens twice gets the same error
    // whether or not the loop has ended in between.
    let (requests, writer, reader, _) = fixture();
    writer.open(desc()).expect("the first open");
    assert!(reader.take().open.is_some());
    reader.end();
    assert_eq!(writer.open(desc()), Err(OpenError::AlreadyOpened));
    assert_eq!(requests.discarded(), 0, "an error is not a discarded write");
    writer.set_clear([0.5; 3]).expect("returns at once");
    assert_eq!(requests.discarded(), 1);

    // No open before the end: the first open is discarded and counted.
    let (requests, writer, reader, _) = fixture();
    reader.end();
    writer.open(desc()).expect("returns at once");
    assert_eq!(requests.discarded(), 1);
    assert_eq!(reader.take(), RequestChanges::default());
}

#[test]
fn every_accepted_write_wakes_and_a_refused_one_does_not() {
    let (_, writer, _, notifier) = fixture();
    assert!(!notifier.take_pending());
    writer.open(desc()).expect("valid");
    assert!(notifier.take_pending(), "open wakes");
    writer.set_clear([0.0; 3]).expect("valid");
    assert!(notifier.take_pending(), "set_clear wakes");
    writer.close();
    assert!(notifier.take_pending(), "close wakes");
    writer.close();
    assert!(notifier.take_pending(), "even a repeated close wakes");
    assert!(writer.set_clear([f32::INFINITY, 0.0, 0.0]).is_err());
    assert!(writer.open(WindowDesc { width: 0, ..desc() }).is_err());
    assert!(
        !notifier.take_pending(),
        "refused writes record nothing and wake nobody"
    );
}

/// A native wake that writes to the slot when it is called: if the slot's lock were held across the
/// notifier's wake, this would deadlock.
struct WritesBack {
    writer: ck_window::RequestWriter,
    calls: Mutex<usize>,
}

impl NativeWake for WritesBack {
    fn wake(&self) {
        let mut calls = self.calls.lock().expect("not poisoned");
        *calls += 1;
        if *calls == 1 {
            drop(calls);
            self.writer.set_clear([0.5; 3]).expect("finite");
        }
    }
}

#[test]
fn no_write_holds_the_slot_while_it_wakes_the_main_thread() {
    owned(
        module_path!(),
        "no_write_holds_the_slot_while_it_wakes_the_main_thread",
        SHORT,
        || {
            let notifier = Notifier::new();
            let requests = Requests::new();
            let writer = requests.writer(Arc::clone(&notifier));
            let target = Arc::new(WritesBack {
                writer: writer.clone(),
                calls: Mutex::new(0),
            });
            notifier
                .arm(Arc::clone(&target) as Arc<dyn NativeWake>)
                .expect("inactive");
            let begun = Instant::now();
            let done = thread::scope(|scope| {
                let handle = scope.spawn(|| writer.set_clear([0.25; 3]));
                // The re-entrant write happens inside the wake, with the slot free.
                let result = handle.join().expect("no panic");
                (result, begun.elapsed())
            });
            assert!(done.0.is_ok());
            assert!(done.1 < BOUND, "a write blocked: {:?}", done.1);
            assert_eq!(
                *target.calls.lock().expect("not poisoned"),
                2,
                "the nested write woke too"
            );
            assert_eq!(
                requests.reader().take().clear,
                Some([0.5; 3]),
                "the later write won"
            );
        },
    );
}

#[test]
fn writers_are_never_blocked_by_a_main_thread_that_is_busy_elsewhere() {
    owned(
        module_path!(),
        "writers_are_never_blocked_by_a_main_thread_that_is_busy_elsewhere",
        SHORT,
        || {
            let (requests, writer, reader, _) = fixture();
            let busy = thread::scope(|scope| {
                // The "main thread": inside a long native call, not reading the slot at all.
                let main = scope.spawn(|| thread::sleep(Duration::from_millis(300)));
                let begun = Instant::now();
                for step in 0..10_000_u16 {
                    writer
                        .set_clear([f32::from(step % 100) / 100.0, 0.0, 0.0])
                        .expect("finite");
                }
                writer.close();
                let took = begun.elapsed();
                main.join().expect("no panic");
                took
            });
            assert!(
                busy < Duration::from_millis(250),
                "writes waited for the main thread: {busy:?}"
            );
            assert!(reader.take().close);
            assert_eq!(requests.discarded(), 0);
        },
    );
}

#[test]
fn dimensions_and_colours_are_validated_and_clamped() {
    let (_, writer, reader, _) = fixture();
    // Invalid input followed by a valid first open: the invalid ones record nothing.
    for bad in [
        WindowDesc { width: 0, ..desc() },
        WindowDesc {
            height: 0,
            ..desc()
        },
        WindowDesc {
            width: MAX_DIMENSION + 1,
            ..desc()
        },
        WindowDesc {
            height: 16_385,
            ..desc()
        },
        WindowDesc {
            clear: [f32::NAN, 0.0, 0.0],
            ..desc()
        },
        WindowDesc {
            clear: [0.0, f32::INFINITY, 0.0],
            ..desc()
        },
        WindowDesc {
            clear: [0.0, 0.0, f32::NEG_INFINITY],
            ..desc()
        },
    ] {
        assert!(
            matches!(writer.open(bad.clone()), Err(OpenError::InvalidDesc { .. })),
            "{bad:?}"
        );
        assert_eq!(
            reader.take(),
            RequestChanges::default(),
            "{bad:?} recorded nothing"
        );
    }
    for edge in [1, MAX_DIMENSION] {
        let (_, writer, reader, _) = fixture();
        writer
            .open(WindowDesc {
                width: edge,
                height: edge,
                ..desc()
            })
            .expect("the boundary is valid");
        assert_eq!(
            reader.take().open.map(|d| (d.width, d.height)),
            Some((edge, edge))
        );
    }
    // A finite colour outside 0..=1 is clamped, in open and in set_clear.
    writer
        .open(WindowDesc {
            clear: [-3.0, 0.5, 9.0],
            ..desc()
        })
        .expect("finite colours are valid");
    assert_eq!(reader.take().open.map(|d| d.clear), Some([0.0, 0.5, 1.0]));
    writer.set_clear([2.0, -2.0, 0.25]).expect("finite");
    assert_eq!(reader.take().clear, Some([1.0, 0.0, 0.25]));
    // A non-finite component is refused with its index, and the last good colour stands.
    assert_eq!(
        writer.set_clear([0.1, f32::NAN, 0.3]),
        Err(InvalidColour { component: 1 })
    );
    assert_eq!(reader.take(), RequestChanges::default());
}

#[test]
fn settings_reject_zero_caps_and_zero_deadlines() {
    let ok = Duration::from_millis(5);
    assert!(LoopSettings::new(1, 1, ok, ok).is_ok());
    for (settings, name) in [
        (LoopSettings::new(0, 1, ok, ok), "native_events"),
        (LoopSettings::new(1, 0, ok, ok), "items"),
        (
            LoopSettings::new(1, 1, Duration::ZERO, ok),
            "presenter_teardown",
        ),
        (LoopSettings::new(1, 1, ok, Duration::ZERO), "disarm"),
    ] {
        assert_eq!(settings.expect_err(name).setting, name);
    }
    let defaults = LoopSettings::default();
    assert_eq!((defaults.native_events(), defaults.items()), (64, 16));
    assert_eq!(defaults.presenter_teardown(), Duration::from_secs(2));
    assert_eq!(defaults.disarm(), Duration::from_millis(100));
}

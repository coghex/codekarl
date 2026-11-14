//! The resizable window (milestone 9, slice 12b): the resize mask, the full-screen behaviour and the content minimum, one size report for a
//! whole live resize and one for each zoom, full-screen transitions that report when they finish, Control-Command-F as a shortcut that is
//! not a key event, and the minimum held through a change of the display's scale. The native delegate calls are simulated through
//! `tests/native/testing.m`, as the other windowless cases' are; no test window really enters full screen.

use ck_input::Key;
use ck_window::{Backend, PixelRequest, WindowDesc};

use crate::support;
use crate::{Fixture, close};

const CONTROL: u64 = 1 << 18;
const COMMAND: u64 = 1 << 20;
/// The macOS virtual key code of F.
const KEY_F: u16 = 3;

/// A whole number of points or pixels from a measure that is one.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a non-negative measure of a window, rounded"
)]
fn whole(measure: f64) -> u32 {
    measure.round() as u32
}

/// A resizable window of `size` points with `min` as its content minimum.
fn resizable(size: (u32, u32), min: (u32, u32)) -> WindowDesc {
    WindowDesc {
        title: "Codekarl resizable window test".to_owned(),
        width: size.0,
        height: size.1,
        clear: [0.0, 0.0, 0.0],
        resizable: true,
        min: Some(min),
        ..WindowDesc::default()
    }
}

/// A resizable window opens with the resize mask, primary full screen and its minimum; a fixed one has neither; both show the drawable
/// anchored to the top left.
pub(crate) fn a_resizable_window_has_the_mask_the_full_screen_behaviour_and_its_minimum() {
    let fixture = Fixture::open_desc(&resizable((640, 400), (320, 200)));
    let info = support::info();
    assert!(info.behaviour.resizable, "{info:?}");
    assert!(
        info.resizing.full_screen_primary,
        "macOS standard full screen"
    );
    assert!(
        close(info.content_min.0, 320.0) && close(info.content_min.1, 200.0),
        "{info:?}"
    );
    assert!(
        info.resizing.gravity_top_left,
        "the drawable is anchored top left"
    );
    assert!(info.style.titled && info.style.closable && info.style.miniaturizable);
    drop(fixture);
    let fixed = Fixture::open(320, 200);
    let info = support::info();
    assert!(
        !info.behaviour.resizable && !info.resizing.full_screen_primary,
        "{info:?}"
    );
    assert!(info.resizing.gravity_top_left);
    drop(fixed);
}

/// Dragging an edge reports nothing until it is let go, and then exactly one size: the final one.
pub(crate) fn a_live_resize_reports_one_size_at_its_end() {
    let mut fixture = Fixture::open_desc(&resizable((640, 400), (320, 200)));
    let scale = support::info().window_scale;
    support::live_resize_begin();
    for size in [(600.0, 380.0), (560.0, 340.0), (500.0, 300.0)] {
        support::resize(size);
        let pumped = fixture.drain();
        assert_eq!(
            (pumped.points, pumped.pixel_size),
            (None, None),
            "no size is reported during the drag: {pumped:?}"
        );
    }
    support::live_resize_end();
    let pumped = fixture.drain();
    assert_eq!(pumped.points, Some((500, 300)), "{pumped:?}");
    assert_eq!(
        pumped.pixel_size,
        Some((whole(500.0 * scale), whole(300.0 * scale))),
        "the pixels are read with the points: {pumped:?}"
    );
    assert_eq!(fixture.drain().points, None, "reported once");
}

/// A resize that is not a drag (a zoom) reports each change.
pub(crate) fn a_resize_outside_a_live_resize_reports_each_change() {
    let mut fixture = Fixture::open_desc(&resizable((640, 400), (320, 200)));
    for size in [(700.0, 450.0), (520.0, 310.0)] {
        support::resize(size);
        let pumped = fixture.drain();
        assert_eq!(
            pumped.points,
            Some((whole(size.0), whole(size.1))),
            "{pumped:?}"
        );
    }
}

/// The backend resize request reaches `AppKit`, preserves the frame's top-left, and reports the final content size once.
pub(crate) fn a_backend_resize_preserves_the_top_left_and_reports_once() {
    let mut fixture = Fixture::open_desc(&resizable((640, 400), (320, 200)));
    let before = support::info();
    fixture.backend.resize_window((720, 480));
    let pumped = fixture.drain();
    let after = support::info();
    assert_eq!(after.content_size, (720.0, 480.0), "{after:?}");
    assert_eq!(
        after.frame_top_left, before.frame_top_left,
        "{before:?} → {after:?}"
    );
    assert_eq!(pumped.points, Some((720, 480)), "{pumped:?}");
    assert_eq!(fixture.drain().points, None, "one size report");
}

/// Entering and leaving full screen are size changes that report when the transition has finished, none during it.
pub(crate) fn full_screen_reports_one_size_when_each_transition_finishes() {
    let mut fixture = Fixture::open_desc(&resizable((640, 400), (320, 200)));
    support::full_screen_begin(true, (1200.0, 700.0));
    assert_eq!(fixture.drain().points, None, "none during the animation");
    support::full_screen_finish(true);
    assert_eq!(fixture.drain().points, Some((1200, 700)), "entered");
    support::full_screen_begin(false, (640.0, 400.0));
    assert_eq!(fixture.drain().points, None, "none while leaving");
    support::full_screen_finish(false);
    assert_eq!(fixture.drain().points, Some((640, 400)), "left");
    assert_eq!(fixture.drain().points, None);
}

/// Control-Command-F reaches `toggleFullScreen:` and is not delivered as a key; Control-F and Command-F alone are keys as before; a
/// window that cannot be resized has no shortcut of ours (`AppKit` may still keep the chord for itself).
pub(crate) fn control_command_f_toggles_full_screen_and_is_not_a_key_event() {
    use crate::support::KeyboardEvent;
    let mut fixture = Fixture::open_desc(&resizable((640, 400), (320, 200)));
    let before = support::toggle_full_screen_calls();
    assert!(support::post_key(KEY_F, CONTROL | COMMAND, false, "f"));
    assert!(support::post_keyboard_event(
        KeyboardEvent::KeyUp,
        KEY_F,
        CONTROL | COMMAND
    ));
    let pumped = fixture.drain();
    assert_eq!(
        support::toggle_full_screen_calls(),
        before + 1,
        "toggleFullScreen: was called once"
    );
    assert_eq!(pumped.keys(), [], "and the game saw no key: {pumped:?}");
    // The same keys without the other modifier are ordinary keys.
    assert!(support::post_key(KEY_F, CONTROL, false, "f"));
    assert!(support::post_key(KEY_F, COMMAND, false, "f"));
    let pumped = fixture.drain();
    assert_eq!(
        pumped.keys(),
        [(Key::KeyF, true, false), (Key::KeyF, true, false)],
        "{pumped:?}"
    );
    assert_eq!(support::toggle_full_screen_calls(), before + 1);
    drop(fixture);
    // A fixed window takes no shortcut of the backend's.
    let mut fixed = Fixture::open(320, 200);
    let before = support::toggle_full_screen_calls();
    assert!(support::post_key(KEY_F, CONTROL | COMMAND, false, "f"));
    let _ = fixed.drain();
    assert_eq!(
        support::toggle_full_screen_calls(),
        before,
        "the backend takes no shortcut for a window that cannot be resized"
    );
}

/// The minimum asked for in pixels holds through a change of the display's scale: the content minimum is recomputed from the pixels, and
/// a window left below it grows to it, keeping its top-left, with one size report for the move and the growth together.
pub(crate) fn a_backing_change_recomputes_the_minimum_and_grows_a_window_below_it() {
    // The window opens at 2 pixels a point: 900x600 points are 1800x1200 pixels, and the minimum of 1280x800 pixels is 640x400 points.
    support::set_scale(2.0);
    let mut desc = resizable((900, 600), (640, 400));
    desc.pixel_request = Some(PixelRequest {
        size: (1800, 1200),
        min: Some((1280, 800)),
    });
    let mut fixture = Fixture::open_desc(&desc);
    let info = support::info();
    assert!(
        close(info.content_min.0, 640.0) && close(info.content_min.1, 400.0),
        "{info:?}"
    );
    let visible = fixture.backend.main_screen().visible_points;
    let wanted = (1280.min(visible.0), 800.min(visible.1));
    // To a 1x display: the minimum is 1280x800 points (or the screen's visible size when that is smaller), and the 900x600 window grows
    // to it.
    support::set_scale(1.0);
    support::backing_changed();
    let pumped = fixture.drain();
    let info = support::info();
    assert!(
        close(info.content_min.0, f64::from(wanted.0))
            && close(info.content_min.1, f64::from(wanted.1)),
        "the minimum is the requested pixels at 1x: {info:?}"
    );
    assert!(
        close(info.content_size.0, f64::from(wanted.0))
            && close(info.content_size.1, f64::from(wanted.1)),
        "the window grew to the minimum: {info:?}"
    );
    assert_eq!(
        pumped.points,
        Some(wanted),
        "one size for the move and the growth: {pumped:?}"
    );
    assert_eq!(
        pumped.pixel_size,
        Some(wanted),
        "at 1 pixel a point: {pumped:?}"
    );
    if wanted != (1280, 800) {
        assert_eq!(
            pumped.min_clamped,
            Some(((1280, 800), wanted)),
            "a screen too small for the minimum says so once: {pumped:?}"
        );
    }
    // Back to 2x: the minimum is 640x400 points again; a window above it is left as it is.
    support::set_scale(2.0);
    support::backing_changed();
    let pumped = fixture.drain();
    let info = support::info();
    assert!(
        close(info.content_min.0, 640.0) && close(info.content_min.1, 400.0),
        "{info:?}"
    );
    assert!(
        close(info.content_size.0, f64::from(wanted.0)),
        "not shrunk: {info:?}"
    );
    assert_eq!(
        pumped.points,
        Some(wanted),
        "the points stay; the pixels moved: {pumped:?}"
    );
    support::set_scale(0.0);
}

//! The native macOS window backend: `AppKit` behind `ck-window`'s [`Backend`](ck_window::Backend).
//!
//! The crate is empty on targets other than macOS. On macOS a small C header and an Objective-C/ARC
//! implementation (`native/`), compiled by `build.rs` with the workspace's one deployment-target rule and
//! the optional Clang static analyzer (`CK_NATIVE_ANALYZE`), sit behind an ownership-safe Rust wrapper.
//! It links `AppKit` and `QuartzCore` only. No Metal device and no drawing live here: the window owns a
//! `CAMetalLayer`, which the Metal presenter uses.
//!
//! # The main thread, and the one exception
//!
//! Every `AppKit` call runs on the process's main thread. [`CocoaBackend`] is neither `Send` nor `Sync`,
//! and [`CocoaBackend::new`] refuses to run anywhere else (checked with `pthread_main_np` before any
//! application object exists). The single exception is the wake: [`Backend::wake_target`](ck_window::Backend::wake_target)
//! returns a `Send + Sync` [`NativeWake`](ck_window::NativeWake) that **retains the application object
//! it captured on the main thread**. Its `wake`, on any thread and inside its own autorelease pool,
//! constructs one application-defined `NSEvent` and calls `postEvent:atStart:` on that retained object,
//! which Apple documents as callable from subthreads
//! (<https://developer.apple.com/documentation/appkit/nsapplication/postevent(_:atstart:)>, and the
//! thread-safety summary in the Cocoa threading guide). Producers never call `sharedApplication` and
//! touch no window, view or layer; the documentation of `postEvent:` does not make any other `AppKit`
//! call thread-safe, and none is made. The wake is **sticky** (an event stays queued, so a
//! `pump(UntilEvent)` entered after it returns at once). While one wake event is queued, further wakes
//! post nothing (they have already set the notifier's pending flag): wake events cannot pile up.
//!
//! **Lifetime.** The wake object holds its own strong reference to the application, so it stays valid
//! for as long as anything holds it. `ck-window`'s notifier stops all wakes at disarm, and the backend
//! drops its own reference only in [`terminate`](ck_window::Backend::terminate), which the loop calls after
//! a successful disarm. Calling `wake` directly on a retained target after teardown is outside the
//! contract; the tests check that wakes made through a disarmed notifier reach no native target.
//!
//! # The custom event loop
//!
//! `ck-window` pumps events itself instead of calling `[NSApp run]`, so the backend does what `run`
//! would: `finishLaunching` once for the process (however many backends are created), activation so the
//! window can take key focus, `updateWindows` after each pass's dispatch, and an autorelease pool around
//! each `pump`, `create_window`, `destroy_window`, `terminate` and each wake. The backend installs no
//! main menu, so Cmd-Q and every other menu key equivalent does nothing. (Observed: `finishLaunching`
//! itself installs an empty placeholder menu, one item with no sub-items and no key equivalent, and
//! `AppKit` puts it back if it is removed; the tests assert that no menu item has a key equivalent.)
//!
//! [`pump`](ck_window::Backend::pump) takes events with `nextEventMatchingMask:untilDate:inMode:dequeue:`
//! (`distantFuture` for the first event only when `wait` is `UntilEvent` and nothing is pending,
//! `distantPast` otherwise and for every later event), dispatches each with `sendEvent:` except this
//! backend's own wake events (consumed), and stops after `cap` events or when none is left. Wake events
//! count toward `cap`.
//!
//! # Facts observed on macOS 26.7 (and why the code is as it is)
//!
//! - A window that was shown stays in `[NSApp windows]` after `close` and release, kept alive by the
//!   show animation, unless its `animationBehavior` is `NSWindowAnimationBehaviorNone`. The backend sets
//!   it, so `destroy_window` leaves no window behind (and the window appears without a fade).
//! - `setActivationPolicy:` fails when it is called again in the same process, so the backend sets the
//!   regular policy only when it is not already in place (several backends may be created in turn).
//! - Key events posted to the application (`postEvent:atStart:`) reach the content view through
//!   `sendEvent:`, which is how the tests drive keys and Cmd-Q.
//! - `-[NSWindow performClose:]` calls `windowShouldClose:` once, and the refusal leaves the window open.
//!
//! # Events: callbacks only record
//!
//! The window delegate, the content view and the dispatch record into a fixed-size pending state and
//! never call back into `ck-window`, start teardown or destroy anything:
//!
//! - the **close button**: `windowShouldClose:` returns `NO` and records a close request, so the window
//!   never closes by itself;
//! - **keys** (milestone 5-6b, design §10.1, §10.2): the content view is the first responder; its `keyDown:`,
//!   `keyUp:` and `flagsChanged:` record every key the table knows and swallow every key, so none beeps or reaches the
//!   next responder (Cmd-Q and the no-menu behaviour stay as they were). Escape is a key like any other here: it is
//!   the `close` binding of `ck-window`'s mapper, no longer a close source of this backend. See below.
//! - **focus**: the delegate's `windowDidResignKey:` and `windowDidBecomeKey:` record a focus loss and gain;
//! - a change of **backing properties** updates the layer's `contentsScale` and records the new pixel
//!   size; a window becoming visible again after occlusion (or deminiaturised) records **needs display**.
//!
//! # Keys: normalised, bounded before Rust
//!
//! One table in the native source maps a hardware key code (macOS virtual key codes of the ANSI positions) to a
//! `ck-input` key; a code outside it is dropped. A modifier's side is decided from the **device-dependent side
//! bits** of `NSEvent.modifierFlags` (`IOKit`'s `NX_DEVICE*KEYMASK`; `NSEventModifierFlag*` is one bit per family and
//! cannot tell a left release while the right stays held): the named side is down when its bit is set, up when clear.
//! With no side bit in the event (some virtual or remote input) the key code toggles the side, and whenever the
//! family's own bit is clear both sides are released, so a missed release can never outlive the family's.
//!
//! The responder records, in constant work and without allocating, into: an authoritative **held-key bitset**
//! (exact); a **ring of 256 ordered key transitions**; and **per-key loss latches** (a press and a release bit per
//! key) that a transition sets only when it is *discarded* from a full ring. A pump hands over, in recorded order and
//! at most its cap: a pending focus loss (not a ring entry, so it cannot be discarded: it clears all key state and is
//! delivered before anything recorded after it), then the ring's transitions as [`Key`
//! events](ck_window::NativeEvent::Key) with a pending focus gain **at the place it was recorded** (a gain is a flag
//! with a position - how many ring records precede it - not a ring entry, so it cannot be discarded and keeps its
//! place across capped pumps). **After an overflow no ordered detail of the span is delivered**: the ring's
//! surviving records follow a lost prefix, so the modifier state they would be evaluated against is unknown; they are
//! discarded into the latches as well and the pump delivers **one** [`KeyResync`](ck_window::NativeEvent::KeyResync)
//! with every key's final held state and the two latches, then clears the latches. The resync stands where its
//! first lost record stood: after a gain recorded before all lost detail, before a gain recorded after some of it.
//! It never synthesises ordered events. Every transition is therefore delivered once or latched once, never both.
//!
//! **The pump hands over before it dispatches.** What it hands over is the key and focus state recorded **before** the
//! pump started; what its own dispatch records (keys, a focus loss, an overflow) belongs to the next pump, stays
//! pending in order and keeps that pump from sleeping. A focus loss recorded during dispatch therefore cannot replace
//! detail that was due to be handed over, and an overflow during dispatch is not folded into the pump's own hand-over.
//!
//! The pending state is the first close source in arrival order with a saturating count of further
//! close-type presses, the latest pixel size, and a needs-display flag. One `pump` hands over at most one
//! close event, one `PixelSizeChanged` and one `NeedsDisplay` (and the key and focus entries above), then clears the
//! state, so the storage is constant however many callbacks one dispatch produces. The absorbed presses are counted and logged
//! once per pump as a `debug` record in category `window.cocoa`.
//!
//! **No sleeping with pending state.** A `pump(UntilEvent)` that starts with pending state does not
//! wait. A callback that runs while `pump` is already waiting (for example from a main-run-loop timer)
//! and moves the state from empty to pending posts one wake event, so the waiting call returns with the
//! work; later callbacks before it returns post nothing more.
//!
//! # Tests while the owner types (CK-63 r5)
//!
//! Window tests open real windows without taking focus, except in explicitly named focus cases. Test workers
//! can suppress production activation requests through `ck-window`'s test-support-only switch; normal builds
//! have no way to set it. The tests
//! give the same result however much is typed. **None of the test support is part of this library.** It
//! lives in `tests/` (`tests/support`, `tests/native`), its native part is built into a separate archive
//! that only the test executable links (`rustc-link-arg-tests` in `build.rs`), and a test checks the
//! library's archive for test symbols. The test executable marks every synthetic keyboard event it posts
//! (`kCGEventSourceUserData` on the `CGEvent` the `NSEvent` is made from, a value real input cannot carry)
//! and installs, before its first window and for its whole life, a filter: a local event monitor, which
//! `AppKit` runs inside `-[NSApplication sendEvent:]` before the event reaches a window, its key
//! equivalents or `keyDown:`. It drops every key-down, key-up and modifier change without the mark, so
//! real typing records nothing, beeps nothing and is not passed on. The tests assert only their own
//! probes (never a count of all keyboard events) and run once more with a thread typing without stopping.
//!
//! # Native modal interactions (design §8)
//!
//! The backend adds no mechanism to escape a nested `AppKit` loop, and the tests do not drive one. By
//! inspection of `AppKit`'s behaviour: a mouse-down in the title bar or on a title-bar button makes
//! `-[NSWindow sendEvent:]` track the mouse in a nested loop until the button is released, so a `pump`
//! that dispatches it does not return until then. A quit requested meanwhile is only recorded by
//! Codekarl's callbacks and takes effect when control returns to the outer loop. Nothing here claims a
//! bound on that wait. A fixed-size window has no live-resize loop.

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod wake;

#[cfg(target_os = "macos")]
pub use backend::{CocoaBackend, CocoaHandles, PumpReport};

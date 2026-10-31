#ifndef CK_WINDOW_COCOA_TESTING_H
#define CK_WINDOW_COCOA_TESTING_H
#include <stdint.h>

/* Test support for this crate's integration test, compiled into a separate archive that only the test
 * executable links (see build.rs). None of it is part of the library. Every function takes the main
 * thread unless it says otherwise. The functions act on the process's one backend window (found through
 * the application's window list), drive AppKit through its real dispatch (posted events, delegate
 * methods, run-loop timers) and never change what the backend does. */

typedef struct {
    int32_t has_window;
    char title[256];
    double content_width;
    double content_height;
    int32_t titled;
    int32_t closable;
    int32_t miniaturizable;
    int32_t resizable;
    int32_t layer_is_metal;
    int32_t layer_is_view_backing_layer;
    double layer_scale;
    double window_scale;
    int32_t visible;
    int32_t is_key;
    int32_t first_responder_is_view;
    int32_t has_delegate;
    int32_t released_when_closed;
    int64_t window_number;
    int32_t has_main_menu;
    uint32_t menu_items;
    uint32_t menu_key_equivalents;
} CKWinTestInfo;

typedef struct {
    uint64_t held[2];
    uint64_t lost_pressed[2];
    uint64_t lost_released[2];
    uint32_t ring_length;
    int32_t overflow;
    int32_t focus_lost_pending;
    int32_t focus_gained_pending;
    uint64_t overflow_total;
} CKWinNativeKeysInfo;

typedef struct {
    uint64_t markers_dispatched;    /* test events that reached sendEvent: */
    uint64_t wake_events_dispatched; /* wake events that reached sendEvent: (the pump consumes its own) */
    uint64_t update_windows_calls;
    uint64_t finish_launching_calls;
    uint64_t wake_posts;            /* wake events posted to the application */
    uint64_t no_responder;          /* key events that reached the window with no responder (the beep path) */
    uint64_t key_downs_seen;        /* key-downs that reached the content view */
    uint64_t recorded_in_pump;      /* timer callbacks that ran inside a pump */
    uint64_t filtered_keys;         /* unmarked keyboard events the input filter dropped */
    uint64_t pump_update_windows_calls; /* updateWindows calls made directly by ck_win_pump */
} CKWinTestCounters;

/* Makes the application object a counting subclass and puts the counting and the scale-factor override
 * into the window and view classes' methods. Call before the backend is created. */
void ck_win_test_install_application(void);
/* Whether the shared application object exists yet (reads a global; any thread). */
int32_t ck_win_test_app_exists(void);
uint64_t ck_win_test_ns_window_count(void);
void ck_win_test_info(CKWinTestInfo *out);
void ck_win_test_counters(CKWinTestCounters *out);

/* Makes the content view (to_view != 0) or nothing the first responder. */
void ck_win_test_set_first_responder(int32_t to_view);
/* [window performClose:nil] */
void ck_win_test_perform_close(void);

/* Posts a key-down NSEvent made from a CGEvent (postEvent:atStart:NO). A marked event carries the tests'
 * kCGEventSourceUserData mark, which real input cannot; an unmarked one stands for the owner's typing.
 * Returns 0 when it could not be made. */
int32_t ck_win_test_post_key(uint16_t key_code, uint64_t modifier_flags, int32_t repeat,
                             const char *characters, int32_t marked);
/* The other keyboard events: kind 1 is a key-up, kind 2 a modifier change. */
int32_t ck_win_test_post_keyboard_event(int32_t kind, uint16_t key_code, uint64_t modifier_flags,
                                        int32_t marked);
/* Installs, once for the process, the filter that drops every unmarked keyboard event (a local event
 * monitor). Main thread. */
void ck_win_test_install_input_filter(void);
/* Starts a thread that types without stopping: unmarked key-downs (Escape among them), repeats, key-ups
 * and modifier changes, posted to the application. Call after the filter is installed. */
void ck_win_test_start_typist(void);

/* Posts an identifiable application-defined test event, at the front of the queue or the back. */
void ck_win_test_post_marker(int64_t id, int32_t at_start);
/* Posts a test event from a main-run-loop timer after the delay, so a blocked pump always ends. */
void ck_win_test_post_marker_after(double delay_seconds, int64_t id);
/* Removes the backend's own wake events from the queue (other application-defined events keep their
 * order); returns how many it removed. */
uint64_t ck_win_test_discard_wake_events(void);
/* Takes and dispatches every queued event without handing over or clearing the pending state. */
void ck_win_test_drain_events(void);

/* Uses `scale` in place of the window's backing scale factor (0 restores it). */
void ck_win_test_set_scale(double scale);
/* Makes the window's delegate see a backing-properties change / the window come back from the Dock. */
void ck_win_test_backing_changed(void);
void ck_win_test_deminiaturized(void);

/* Calls the recording callbacks directly, in one go, as a burst inside one dispatch. */
void ck_win_test_burst(uint32_t button_presses, uint32_t pixel_changes, uint32_t needs_display);
/* Records `pairs` key-down / key-up pairs of the key code directly (no event is posted), as one burst inside one
 * dispatch: the fast way to overflow the ring. Returns 0 when the code is not in the key table. */
int32_t ck_win_test_record_keys(uint32_t pairs, uint16_t key_code);
void ck_win_test_set_absorbed(uint32_t count);
/* Schedules `timers` default-mode timers on the main run loop, all due after the delay; each calls the
 * recording callback `calls` times (kind 1: button close, 3: needs display). */
void ck_win_test_schedule_records(double delay_seconds, uint32_t timers, uint32_t calls, int32_t kind);
/* Posts a marker whose dispatch, inside a pump, records input: kind 0 `pairs` down/up pairs of `key_code`, 1 a focus
 * loss, 2 a focus gain. */
void ck_win_test_post_recording_marker(int32_t kind, uint32_t pairs, uint16_t key_code);

/* The native key state: what the responder recorded and has not handed over (the held bitset, the loss latches,
 * the ring's length, the overflow flag, the pending focus events). */
void ck_win_test_native_keys(CKWinNativeKeysInfo *out);
/* One entry of the native key table (position 0..count-1): the code, the key number and the W3C name. Returns 0 past
 * the end. */
int32_t ck_win_test_key_table(uint32_t position, uint16_t *key_code, uint8_t *key, char *name,
                              uint32_t name_capacity);
uint32_t ck_win_test_key_table_count(void);
/* The window resigns / becomes key, through the notifications AppKit sends its delegate. */
void ck_win_test_resign_key(void);
void ck_win_test_become_key(void);

/* Calls a main-thread-only AppKit method from a thread that is not the main thread (the control for the
 * Main Thread Checker). */
void ck_win_test_violate_main_thread(void);

#endif

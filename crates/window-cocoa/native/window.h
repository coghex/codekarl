#ifndef CK_WINDOW_COCOA_H
#define CK_WINDOW_COCOA_H
#include <stddef.h>
#include <stdint.h>

/* The native window backend. One CKWinHandle is the backend (the application, its one window and the
 * bounded pending state); one CKWinWakeHandle is a retained reference to the application's wake target.
 *
 * Every function takes the main thread, except ck_win_wake and ck_win_wake_release, which are the
 * crate's only off-main operations: they post an application-defined event to the retained application
 * object and touch nothing else.
 */
typedef struct CKWinHandle CKWinHandle;
typedef struct CKWinWakeHandle CKWinWakeHandle;

#define CK_WIN_CLOSE_NONE 0
#define CK_WIN_CLOSE_BUTTON 1

/* The supported physical keys: the positions of ck-input's `Key::ALL` (81 keys). A key is a number 0..80 here. */
#define CK_WIN_KEY_COUNT 81
/* The ring of ordered key transitions (the detail), and the most one pump can hand over: the whole ring, a
 * focus loss and a focus gain, or one resync (which replaces the ring). */
#define CK_WIN_RING_CAPACITY 256
#define CK_WIN_OUT_MAX (CK_WIN_RING_CAPACITY + 2)
#define CK_WIN_POINTER_CAPACITY 64
/* The typed entries one pump can hand over (CK-314): the native store is this big, and drops the later entries past it. */
#define CK_WIN_TYPED_CAPACITY 256

/* The kinds of a typed entry. */
#define CK_WIN_TYPED_CHAR 0       /* a committed character: `value` is its Unicode scalar value */
#define CK_WIN_TYPED_BACKSPACE 1
#define CK_WIN_TYPED_ENTER 2
#define CK_WIN_TYPED_TAB 3
#define CK_WIN_TYPED_LEFT 4
#define CK_WIN_TYPED_RIGHT 5

typedef struct {
    uint8_t kind;    /* CK_WIN_TYPED_* */
    uint32_t value;  /* the scalar value, for CK_WIN_TYPED_CHAR */
} CKWinTyped;

/* The kinds of what a pump hands over, in recorded order. */
#define CK_WIN_OUT_KEY 0          /* a key transition: key, down, repeat */
#define CK_WIN_OUT_FOCUS_LOST 1
#define CK_WIN_OUT_FOCUS_GAINED 2
#define CK_WIN_OUT_RESYNC 3       /* the ring overflowed: see CKWinResync */

typedef struct {
    uint8_t kind;    /* CK_WIN_OUT_* */
    uint8_t key;     /* 0..CK_WIN_KEY_COUNT-1, for CK_WIN_OUT_KEY */
    uint8_t down;
    uint8_t repeat;
} CKWinOut;

/* After an overflow, in place of any ordered detail: for every supported key (bit n % 64 of word n / 64) its final
 * held state, and the two loss latches (pressed / released in a discarded transition). */
typedef struct {
    uint64_t held[2];
    uint64_t pressed[2];
    uint64_t released[2];
} CKWinResync;

typedef struct {
    uint8_t kind; /* 0 down, 1 move, 2 up, 3 scroll, 4 non-left down, 5 non-left up */
    uint8_t unit; /* 0 lines, 1 points */
    uint8_t button; /* 0 none, 1 right, 2 middle */
    int32_t x;    /* 1/256 point, origin top-left */
    int32_t y;
    int32_t amount; /* 1/256 selected scroll unit */
} CKWinPointer;

/* What one pump hands over: the bounded pending state, taken and cleared. */
typedef struct {
    int32_t close_source;     /* CK_WIN_CLOSE_*: the first close in arrival order */
    uint32_t close_absorbed;  /* further close presses, saturating */
    int32_t size_changed;     /* 1 when the content size changed: one report for a live resize, however long, and for each zoom,
                               * full-screen transition or backing change that is not part of one */
    uint32_t point_width;     /* the latest content size in points, when size_changed */
    uint32_t point_height;
    uint32_t pixel_width;     /* the latest drawable size in pixels, when size_changed */
    uint32_t pixel_height;
    int32_t min_clamped;      /* 1 when a smaller screen lowered the content minimum (the Rust side logs it once) */
    uint32_t min_requested_width;  /* the minimum asked for, in points, and the one used, when min_clamped */
    uint32_t min_requested_height;
    uint32_t min_used_width;
    uint32_t min_used_height;
    int32_t needs_display;    /* 1 when a redraw is needed */
    int32_t minimized;        /* 1 when the window was miniaturized */
    int32_t restored;         /* 1 when the window was deminiaturized */
    uint64_t native_events;   /* events this pump took off the queue, its own wake events included */
    uint64_t overflow_total;  /* transitions discarded into the latches since the backend was made (diagnostics) */
    uint32_t out_count;       /* entries of `out` */
    CKWinOut out[CK_WIN_OUT_MAX];
    CKWinResync resync;       /* the payload of the CK_WIN_OUT_RESYNC entry, when there is one */
    uint32_t pointer_count;
    uint8_t pointer_overflow;
    uint8_t pointer_held;
    int32_t pointer_x;
    int32_t pointer_y;
    CKWinPointer pointers[CK_WIN_POINTER_CAPACITY];
    uint32_t typed_count;     /* entries of `typed`, in the order typed (CK-314) */
    uint8_t typed_lost;       /* 1 when later entries were dropped at the native bound */
    CKWinTyped typed[CK_WIN_TYPED_CAPACITY];
} CKWinEvents;

/* The size of CKWinEvents, so the Rust side can refuse to run with a different layout. */
size_t ck_win_events_size(void);

/* Returns the backend, or NULL and a NUL-terminated message. Refused off the main thread before any
 * application object exists. Calls finishLaunching once for the process. */
CKWinHandle *ck_win_create(char *error, size_t error_capacity);

/* The main screen as the main thread sees it: its scale factor (pixels per point) and its visible area in points.
 * Callable before a window exists. With no screen (a session with no display) it says 1 and 4294967295 for both
 * sizes. */
void ck_win_main_screen(CKWinHandle *handle, double *backing, uint32_t *visible_width, uint32_t *visible_height);

/* Creates the window (title is UTF-8 and may contain NUL bytes): fixed-size unless `resizable`, in which case it can be
 * resized, zoomed and made full screen (the green button and Control-Command-F), and `content_min_*` (points; 0 for none)
 * is its content minimum. `pixel_min_*` (pixels; 0 for none) is the minimum as requested in pixels: when given it
 * decides the content minimum at the window's backing scale, now and at every change of it, and a window below the
 * minimum after a change is grown to it. `clear` is the colour the window shows where the drawable does not cover it.
 * Returns 0 and the layer pointer (valid until ck_win_destroy_window), the initial content size in points and the
 * drawable's size in pixels, or 1 and a message with no window left behind. */
int ck_win_create_window(CKWinHandle *handle, const char *title, size_t title_length,
                         uint32_t width, uint32_t height, int activate, int ignore_mouse, int resizable,
                         uint32_t content_min_width, uint32_t content_min_height,
                         uint32_t pixel_min_width, uint32_t pixel_min_height, const float *clear,
                         void **layer, uint32_t *point_width, uint32_t *point_height,
                         uint32_t *pixel_width, uint32_t *pixel_height,
                         char *error, size_t error_capacity);

/* Takes events with the caller's cap, dispatches them with sendEvent:, consumes this backend's wake
 * events, runs updateWindows and hands over the pending state: at most `cap` of the key and focus entries
 * (a resync is one entry, never split), in recorded order; what the cap leaves waits for the next pump.
 * wait != 0 blocks for the first event unless the pending state is not empty. */
void ck_win_pump(CKWinHandle *handle, int wait, size_t cap, CKWinEvents *out);
/* As above, but the first event wait ends at the supplied relative interval from this call. */
void ck_win_pump_until(CKWinHandle *handle, size_t cap, double timeout_seconds, CKWinEvents *out);

void ck_win_destroy_window(CKWinHandle *handle);
/* Idempotent; safe with no window. The shared application stays. */
void ck_win_terminate(CKWinHandle *handle);
/* Consumes the handle (terminates first); NULL is allowed. */
void ck_win_free(CKWinHandle *handle);

/* Turns the typed-entry route on or off (CK-314), on the main thread, outside a pump. Either way it starts from nothing: text being
 * composed and entries not yet handed over are discarded. While it is off no key is given to the system's text input. */
void ck_win_set_text_input(CKWinHandle *handle, int enabled);

/* Returns one retained wake reference for ck_win_wake_release to consume. Main thread. */
CKWinWakeHandle *ck_win_wake_target(CKWinHandle *handle);
/* Any thread: posts one application-defined event (coalesced while one is queued). */
void ck_win_wake(const CKWinWakeHandle *wake);
void ck_win_wake_release(CKWinWakeHandle *wake);

#endif

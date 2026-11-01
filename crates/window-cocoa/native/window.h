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

/* What one pump hands over: the bounded pending state, taken and cleared. */
typedef struct {
    int32_t close_source;     /* CK_WIN_CLOSE_*: the first close in arrival order */
    uint32_t close_absorbed;  /* further close presses, saturating */
    int32_t pixel_changed;    /* 1 when the pixel size changed */
    uint32_t pixel_width;     /* the latest pixel size, when pixel_changed */
    uint32_t pixel_height;
    int32_t needs_display;    /* 1 when a redraw is needed */
    int32_t minimized;        /* 1 when the window was miniaturized */
    int32_t restored;         /* 1 when the window was deminiaturized */
    uint64_t native_events;   /* events this pump took off the queue, its own wake events included */
    uint64_t overflow_total;  /* transitions discarded into the latches since the backend was made (diagnostics) */
    uint32_t out_count;       /* entries of `out` */
    CKWinOut out[CK_WIN_OUT_MAX];
    CKWinResync resync;       /* the payload of the CK_WIN_OUT_RESYNC entry, when there is one */
} CKWinEvents;

/* The size of CKWinEvents, so the Rust side can refuse to run with a different layout. */
size_t ck_win_events_size(void);

/* Returns the backend, or NULL and a NUL-terminated message. Refused off the main thread before any
 * application object exists. Calls finishLaunching once for the process. */
CKWinHandle *ck_win_create(char *error, size_t error_capacity);

/* Creates the fixed-size window (title is UTF-8 and may contain NUL bytes). Returns 0 and the layer
 * pointer (valid until ck_win_destroy_window) and the initial pixel size, or 1 and a message with no
 * window left behind. */
int ck_win_create_window(CKWinHandle *handle, const char *title, size_t title_length,
                         uint32_t width, uint32_t height, int activate, void **layer,
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

/* Returns one retained wake reference for ck_win_wake_release to consume. Main thread. */
CKWinWakeHandle *ck_win_wake_target(CKWinHandle *handle);
/* Any thread: posts one application-defined event (coalesced while one is queued). */
void ck_win_wake(const CKWinWakeHandle *wake);
void ck_win_wake_release(CKWinWakeHandle *wake);

#endif

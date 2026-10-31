#ifndef CK_WINDOW_GLFW_TESTING_H
#define CK_WINDOW_GLFW_TESTING_H
#include <stddef.h>
#include <stdint.h>

/* Test support for this crate's integration test, compiled into a separate archive that only the test
 * executable links (see build.rs). None of it is part of the library. Every function takes the main
 * thread unless it says otherwise. The functions act on the process's one GLFW window (found through the
 * application's window list), drive AppKit through its real dispatch (posted events, delegate methods and
 * notifications) and never change what the backend does. */

typedef struct {
    int32_t has_window;
    char title[256];
    double content_width;
    double content_height;
    int32_t resizable;
    int32_t visible;
    int32_t is_key;
    double window_scale;
    int32_t has_main_menu;
    uint32_t menu_items;
    uint32_t menu_key_equivalents;
    uint64_t window_count;
    uint64_t filtered_keys; /* unmarked keyboard events the input filter dropped */
    int32_t activation_policy; /* NSApplicationActivationPolicy of the shared application; -1 before it exists */
    int32_t is_active;         /* [NSApp isActive] */
} CKGlfwTestInfo;

void ck_glfw_test_info(CKGlfwTestInfo *out);

/* The layout CKGlfwTestInfo was compiled with, for every Rust consumer to compare with its own binding
 * before its first ck_glfw_test_info call (a mismatched binding makes that call write outside its
 * object). The fields, in declaration order. */
#define CK_GLFW_TEST_INFO_FIELDS(X) \
    X(has_window) X(title) X(content_width) X(content_height) X(resizable) X(visible) X(is_key) \
    X(window_scale) X(has_main_menu) X(menu_items) X(menu_key_equivalents) X(window_count) \
    X(filtered_keys) X(activation_policy) X(is_active)

/* Writes `sizeof` and `_Alignof` of CKGlfwTestInfo, then the offset of each field in the order above, to
 * `out`, at most `capacity` values, and returns how many values there are (so a caller with too small a
 * buffer, or a different idea of the field list, can tell). Touches no window and no AppKit; any thread. */
size_t ck_glfw_test_info_layout(uint64_t *out, size_t capacity);

/* Activates the application and makes the window key, so posted key events reach it. */
void ck_glfw_test_activate(void);
/* [window performClose:nil]: what the close button does (the delegate's windowShouldClose:). */
void ck_glfw_test_perform_close(void);

/* Posts a key-down NSEvent made from a CGEvent (postEvent:atStart:NO). A marked event carries the tests'
 * kCGEventSourceUserData mark, which real input cannot; an unmarked one stands for the owner's typing.
 * Returns 0 when it could not be made. Any thread. */
int32_t ck_glfw_test_post_key(uint16_t key_code, uint64_t modifier_flags, int32_t repeat,
                              const char *characters, int32_t marked);
/* The other keyboard events: kind 1 is a key-up, kind 2 a modifier change. Any thread. */
int32_t ck_glfw_test_post_keyboard_event(int32_t kind, uint16_t key_code, uint64_t modifier_flags,
                                         int32_t marked);
/* Installs, once for the process, the filter that drops every unmarked keyboard event (a local event
 * monitor). Main thread, before the first window. */
void ck_glfw_test_install_input_filter(void);
/* Starts a thread that types without stopping: unmarked key-downs (Escape among them), repeats, key-ups
 * and modifier changes, posted to the application. Call after the filter is installed. */
void ck_glfw_test_start_typist(void);

/* Starts counting the calls to -[NSApplication setActivationPolicy:] and, while `refuse` is nonzero, makes
 * each one return NO without changing the policy (the failure the library must report). Idempotent; main
 * thread, before the backend is created. */
void ck_glfw_test_watch_policy(int32_t refuse);
/* How many times setActivationPolicy: was called since ck_glfw_test_watch_policy first ran. */
uint64_t ck_glfw_test_policy_calls(void);

/* Starts counting the calls to -[NSApplication activate] (macOS 14 and later; GLFW itself uses
 * activateIgnoringOtherApps:, so a call to this one is the backend's own request). Returns 0 when the
 * selector does not exist on this system. Idempotent; main thread. */
int32_t ck_glfw_test_watch_activate(void);
/* How many times activate was called since ck_glfw_test_watch_activate first ran. */
uint64_t ck_glfw_test_activate_calls(void);

/* The window's number (0 when there is none), so another thread can address an event to it. Main thread. */
int64_t ck_glfw_test_window_number(void);
/* Posts a mouse-moved event for the window `window_number`, as the window system does when a window is
 * ordered or activated under the pointer: a system event that ends a blocking wait with nothing for the
 * backend to hand over. Any thread. */
void ck_glfw_test_post_mouse_moved(int64_t window_number);

/* A trace of what the application receives, for diagnosing a wait that ended early: every event AppKit
 * dispatches to the application (type and subtype) and every notification posted on the default center, each
 * with the milliseconds since the trace started. Test-only; it changes nothing the backend does. */
void ck_glfw_test_trace_start(void);
/* Adds a line of the caller's own to the trace (a marker between steps). Any thread. */
void ck_glfw_test_trace_mark(const char *text);
/* Copies the trace so far, one line per entry, into `out` (NUL-terminated, at most `capacity` bytes) and
 * clears it; returns the number of bytes written without the NUL. Any thread. */
size_t ck_glfw_test_trace_take(char *out, size_t capacity);

/* Uses `scale` in place of the window's backing scale factor (0 restores it). */
void ck_glfw_test_set_scale(double scale);
/* Makes AppKit tell the window's views that the backing properties changed, as a move between displays
 * with different scale factors does. */
void ck_glfw_test_backing_changed(void);

#endif

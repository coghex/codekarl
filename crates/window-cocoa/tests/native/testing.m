#import "../../native/internal.h"
#import <objc/runtime.h>
#include <execinfo.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include "testing.h"
#include "activation_watch.inc"

/* The subtype of the identifiable events the tests post, and the mark their synthetic keyboard events
 * carry in kCGEventSourceUserData (real input carries 0). */
#define CK_TEST_SUBTYPE 0x5445
#define CK_TEST_INPUT_MARK 0x434B54455354LL
/* A marker with this id makes its own dispatch call updateWindows: AppKit's kind of call, from inside a pump. */
#define CK_TEST_UPDATE_MARKER 0x55504454
/* A marker with this id records input (see `ck_win_test_post_recording_marker`) while it is dispatched. */
#define CK_TEST_RECORD_MARKER 0x52454344

static _Atomic uint64_t ck_markers;
static _Atomic uint64_t ck_wake_dispatched;
static _Atomic uint64_t ck_updates;
static _Atomic uint64_t ck_pump_updates;
static _Atomic uint64_t ck_finishes;
static _Atomic uint64_t ck_wake_posts;
static _Atomic uint64_t ck_no_responder;
static _Atomic uint64_t ck_key_downs;
static _Atomic uint64_t ck_in_pump;
static _Atomic uint64_t ck_filtered_keys;
static _Atomic double ck_scale_override;

static BOOL ck_is_wake_event(NSEvent *event) {
    return event.type == NSEventTypeApplicationDefined && event.subtype == CK_WAKE_SUBTYPE &&
           event.data1 == CK_WAKE_MAGIC;
}

/* ---------------------------------------------------------------------------------------------
 * Counting subclasses, swapped in by the tests only
 * ------------------------------------------------------------------------------------------- */

static CKWinState *ck_state(void);
static NSWindow *ck_window(void);

@interface CKTestApplication : NSApplication
@end

@implementation CKTestApplication

- (void)sendEvent:(NSEvent *)event {
    if (event.type == NSEventTypeApplicationDefined && event.subtype == CK_TEST_SUBTYPE) {
        atomic_fetch_add(&ck_markers, 1);
        if (event.data1 == CK_TEST_UPDATE_MARKER) {
            [self updateWindows];
        }
        if (event.data1 == CK_TEST_RECORD_MARKER) {
            uint64_t what = (uint64_t)event.data2;
            uint32_t kind = (uint32_t)(what & 0xff);
            uint32_t pairs = (uint32_t)((what >> 8) & 0xffffff);
            uint16_t key_code = (uint16_t)(what >> 32);
            CKWinState *state = ck_state();
            if (kind == 1) {
                [state recordFocusLost];
            } else if (kind == 2) {
                [state recordFocusGained];
            } else {
                for (uint32_t i = 0; i < pairs; i++) {
                    [state recordKeyCode:key_code down:YES repeat:NO];
                    [state recordKeyCode:key_code down:NO repeat:NO];
                }
            }
        }
    }
    if (ck_is_wake_event(event)) {
        atomic_fetch_add(&ck_wake_dispatched, 1);
    }
    [super sendEvent:event];
}

- (void)updateWindows {
    atomic_fetch_add(&ck_updates, 1);
    /* AppKit calls updateWindows itself too (while dispatching activation or window-ordering events, which
     * the owner's focus changes cause), so the pump's own calls are told apart by their direct caller:
     * frame 1, because objc_msgSend tail-calls this method from the caller's frame. */
    void *frames[2];
    if (backtrace(frames, 2) == 2) {
        char **symbols = backtrace_symbols(frames + 1, 1);
        if (symbols != NULL) {
            if (strstr(symbols[0], " ck_win_pump + ") != NULL ||
                strstr(symbols[0], " ck_win_pump_impl + ") != NULL) {
                atomic_fetch_add(&ck_pump_updates, 1);
            }
            free(symbols);
        }
    }
    [super updateWindows];
}

- (void)finishLaunching {
    atomic_fetch_add(&ck_finishes, 1);
    [super finishLaunching];
}

/* Any thread. */
- (void)postEvent:(NSEvent *)event atStart:(BOOL)atStart {
    if (ck_is_wake_event(event)) {
        atomic_fetch_add(&ck_wake_posts, 1);
    }
    [super postEvent:event atStart:atStart];
}

@end

/* Counting and the scale-factor override are put in by replacing methods (once, when the test application
 * is installed), so the window and view the backend creates are the ones it creates in production. */
static IMP ck_original_scale_factor;
static IMP ck_original_no_responder;
static IMP ck_original_key_down;

static void ck_instrument_classes(void) {
    Method scale = class_getInstanceMethod([NSWindow class], @selector(backingScaleFactor));
    ck_original_scale_factor = method_getImplementation(scale);
    method_setImplementation(scale, imp_implementationWithBlock(^CGFloat(NSWindow *window) {
        double override = atomic_load(&ck_scale_override);
        return override > 0 ? override
                            : ((CGFloat(*)(id, SEL))ck_original_scale_factor)(
                                  window, @selector(backingScaleFactor));
    }));
    Method none = class_getInstanceMethod([NSWindow class], @selector(noResponderFor:));
    ck_original_no_responder = method_getImplementation(none);
    method_setImplementation(none, imp_implementationWithBlock(^(NSWindow *window, SEL selector) {
        atomic_fetch_add(&ck_no_responder, 1);
        ((void (*)(id, SEL, SEL))ck_original_no_responder)(window, @selector(noResponderFor:),
                                                            selector);
    }));
    Method key = class_getInstanceMethod([CKWinView class], @selector(keyDown:));
    ck_original_key_down = method_getImplementation(key);
    method_setImplementation(key, imp_implementationWithBlock(^(CKWinView *view, NSEvent *event) {
        atomic_fetch_add(&ck_key_downs, 1);
        ((void (*)(id, SEL, id))ck_original_key_down)(view, @selector(keyDown:), event);
    }));
}

/* ---------------------------------------------------------------------------------------------
 * Finding the backend
 * ------------------------------------------------------------------------------------------- */

static NSWindow *ck_window(void) {
    return NSApp.windows.firstObject;
}

/* The backend's state, through its window's delegate. */
static CKWinState *ck_state(void) {
    id delegate = ck_window().delegate;
    return [delegate isKindOfClass:[CKWinDelegate class]] ? ((CKWinDelegate *)delegate).state : nil;
}

void ck_win_test_install_application(void) {
    @autoreleasepool {
        static dispatch_once_t once;
        dispatch_once(&once, ^{
            ck_instrument_classes();
        });
        [CKTestApplication sharedApplication];
    }
}

int32_t ck_win_test_app_exists(void) {
    /* NSApp is the global the first sharedApplication call sets: nil until an application object exists. */
    return NSApp != nil ? 1 : 0;
}

/* Counts the items under `menu` and those with a key equivalent. */
static void ck_count_menu(NSMenu *menu, uint32_t *items, uint32_t *keys) {
    for (NSMenuItem *item in menu.itemArray) {
        *items += 1;
        if (item.keyEquivalent.length != 0) {
            *keys += 1;
        }
        if (item.submenu != nil) {
            ck_count_menu(item.submenu, items, keys);
        }
    }
}

uint64_t ck_win_test_ns_window_count(void) {
    @autoreleasepool {
        return (uint64_t)NSApp.windows.count;
    }
}

void ck_win_test_info(CKWinTestInfo *out) {
    @autoreleasepool {
        memset(out, 0, sizeof *out);
        out->has_main_menu = NSApp.mainMenu != nil ? 1 : 0;
        if (NSApp.mainMenu != nil) {
            ck_count_menu(NSApp.mainMenu, &out->menu_items, &out->menu_key_equivalents);
        }
        NSWindow *window = ck_window();
        if (window == nil) {
            return;
        }
        out->has_window = 1;
        snprintf(out->title, sizeof out->title, "%s", window.title.UTF8String ?: "");
        NSRect content = [window contentRectForFrameRect:window.frame];
        out->content_width = content.size.width;
        out->content_height = content.size.height;
        NSWindowStyleMask mask = window.styleMask;
        out->titled = (mask & NSWindowStyleMaskTitled) != 0;
        out->closable = (mask & NSWindowStyleMaskClosable) != 0;
        out->miniaturizable = (mask & NSWindowStyleMaskMiniaturizable) != 0;
        out->resizable = (mask & NSWindowStyleMaskResizable) != 0;
        CALayer *layer = window.contentView.layer;
        out->layer_is_metal = [layer isKindOfClass:[CAMetalLayer class]] ? 1 : 0;
        out->layer_is_view_backing_layer = window.contentView.wantsLayer && layer != nil ? 1 : 0;
        out->layer_scale = layer.contentsScale;
        out->window_scale = window.backingScaleFactor;
        out->visible = window.visible ? 1 : 0;
        out->is_key = window.keyWindow ? 1 : 0;
        out->first_responder_is_view = window.firstResponder == window.contentView ? 1 : 0;
        out->has_delegate = window.delegate != nil ? 1 : 0;
        out->released_when_closed = window.releasedWhenClosed ? 1 : 0;
        out->window_number = (int64_t)window.windowNumber;
    }
}

void ck_win_test_counters(CKWinTestCounters *out) {
    out->markers_dispatched = atomic_load(&ck_markers);
    out->wake_events_dispatched = atomic_load(&ck_wake_dispatched);
    out->update_windows_calls = atomic_load(&ck_updates);
    out->pump_update_windows_calls = atomic_load(&ck_pump_updates);
    out->finish_launching_calls = atomic_load(&ck_finishes);
    out->wake_posts = atomic_load(&ck_wake_posts);
    out->no_responder = atomic_load(&ck_no_responder);
    out->key_downs_seen = atomic_load(&ck_key_downs);
    out->recorded_in_pump = atomic_load(&ck_in_pump);
    out->filtered_keys = atomic_load(&ck_filtered_keys);
}

void ck_win_test_set_first_responder(int32_t to_view) {
    @autoreleasepool {
        NSWindow *window = ck_window();
        [window makeFirstResponder:to_view != 0 ? window.contentView : nil];
    }
}

void ck_win_test_perform_close(void) {
    @autoreleasepool {
        [ck_window() performClose:nil];
    }
}

/* ---------------------------------------------------------------------------------------------
 * Keyboard input: marked synthetic events, the owner's typing, and the filter
 * ------------------------------------------------------------------------------------------- */

/* Makes a keyboard NSEvent from a CGEvent. kind 0 is a key-down, 1 a key-up, 2 a modifier change. */
static NSEvent *ck_keyboard_event(int32_t kind, uint16_t key_code, uint64_t flags, int32_t repeat,
                                  const char *characters, int32_t marked) {
    CGEventRef raw = CGEventCreateKeyboardEvent(NULL, key_code, kind == 0);
    if (raw == NULL) {
        return nil;
    }
    if (kind == 2) {
        CGEventSetType(raw, kCGEventFlagsChanged);
    }
    CGEventSetFlags(raw, (CGEventFlags)flags);
    CGEventSetIntegerValueField(raw, kCGKeyboardEventAutorepeat, repeat != 0 ? 1 : 0);
    CGEventSetIntegerValueField(raw, kCGEventSourceUserData, marked != 0 ? CK_TEST_INPUT_MARK : 0);
    if (characters != NULL) {
        NSString *chars = [NSString stringWithUTF8String:characters];
        UniChar units[8];
        NSUInteger length = MIN(chars.length, (NSUInteger)8);
        [chars getCharacters:units range:NSMakeRange(0, length)];
        CGEventKeyboardSetUnicodeString(raw, (UniCharCount)length, units);
    }
    NSEvent *event = [NSEvent eventWithCGEvent:raw];
    CFRelease(raw);
    return event;
}

int32_t ck_win_test_post_key(uint16_t key_code, uint64_t modifier_flags, int32_t repeat,
                             const char *characters, int32_t marked) {
    @autoreleasepool {
        NSEvent *event = ck_keyboard_event(0, key_code, modifier_flags, repeat, characters, marked);
        if (event == nil) {
            return 0;
        }
        [NSApp postEvent:event atStart:NO];
        return 1;
    }
}

int32_t ck_win_test_post_keyboard_event(int32_t kind, uint16_t key_code, uint64_t modifier_flags,
                                        int32_t marked) {
    @autoreleasepool {
        NSEvent *event = ck_keyboard_event(kind, key_code, modifier_flags, 0, NULL, marked);
        if (event == nil) {
            return 0;
        }
        [NSApp postEvent:event atStart:NO];
        return 1;
    }
}

/* The test-only input filter: a local event monitor, which AppKit runs inside -[NSApplication
 * sendEvent:] before the event reaches a window, its key equivalents or keyDown:. It drops every
 * keyboard event (key down, key up, modifier change) that does not carry the tests' mark, so the
 * owner's typing, Escape included, never reaches a test window. */
static id ck_input_filter = nil;

void ck_win_test_install_input_filter(void) {
    @autoreleasepool {
        if (ck_input_filter != nil) {
            return;
        }
        NSEventMask keyboard = NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged;
        ck_input_filter = [NSEvent addLocalMonitorForEventsMatchingMask:keyboard
                                                                handler:^NSEvent *(NSEvent *event) {
            CGEventRef raw = event.CGEvent;
            if (raw != NULL &&
                CGEventGetIntegerValueField(raw, kCGEventSourceUserData) == CK_TEST_INPUT_MARK) {
                return event;
            }
            atomic_fetch_add(&ck_filtered_keys, 1);
            return nil;
        }];
    }
}

/* The owner, typing without stopping: unmarked events of every keyboard kind, Escape among them. */
void ck_win_test_start_typist(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            for (;;) {
                @autoreleasepool {
                    NSEvent *events[] = {
                        ck_keyboard_event(0, 53, 0, 0, "\x1b", 0),
                        ck_keyboard_event(0, 53, 0, 1, "\x1b", 0),
                        ck_keyboard_event(1, 53, 0, 0, NULL, 0),
                        ck_keyboard_event(0, 0, 0, 0, "a", 0),
                        ck_keyboard_event(2, 55, NSEventModifierFlagCommand, 0, NULL, 0),
                        ck_keyboard_event(0, 12, NSEventModifierFlagCommand, 0, "q", 0),
                    };
                    for (size_t i = 0; i < sizeof events / sizeof events[0]; i++) {
                        if (events[i] != nil) {
                            [NSApp postEvent:events[i] atStart:NO];
                        }
                    }
                }
                usleep(5000);
            }
        });
    });
}

/* ---------------------------------------------------------------------------------------------
 * Identifiable events, the queue, and the recording callbacks
 * ------------------------------------------------------------------------------------------- */

static NSEvent *ck_marker(int64_t id) {
    return [NSEvent otherEventWithType:NSEventTypeApplicationDefined
                              location:NSZeroPoint
                         modifierFlags:0
                             timestamp:0
                          windowNumber:0
                               context:nil
                               subtype:CK_TEST_SUBTYPE
                                 data1:(NSInteger)id
                                 data2:0];
}

void ck_win_test_post_marker(int64_t id, int32_t at_start) {
    @autoreleasepool {
        [NSApp postEvent:ck_marker(id) atStart:at_start != 0];
    }
}

void ck_win_test_post_marker_after(double delay_seconds, int64_t id) {
    @autoreleasepool {
        NSTimer *timer = [NSTimer timerWithTimeInterval:delay_seconds
                                                repeats:NO
                                                  block:^(NSTimer *fired) {
                                                      (void)fired;
                                                      [NSApp postEvent:ck_marker(id) atStart:NO];
                                                  }];
        [[NSRunLoop mainRunLoop] addTimer:timer forMode:NSDefaultRunLoopMode];
    }
}

uint64_t ck_win_test_discard_wake_events(void) {
    @autoreleasepool {
        NSMutableArray<NSEvent *> *kept = [NSMutableArray array];
        uint64_t discarded = 0;
        for (;;) {
            NSEvent *event = [NSApp nextEventMatchingMask:NSEventMaskApplicationDefined
                                                untilDate:[NSDate distantPast]
                                                   inMode:NSDefaultRunLoopMode
                                                  dequeue:YES];
            if (event == nil) {
                break;
            }
            if (ck_is_wake_event(event)) {
                discarded += 1;
            } else {
                [kept addObject:event];
            }
        }
        /* Back to the front, last first, so the kept events keep their order. */
        for (NSEvent *event in [kept reverseObjectEnumerator]) {
            [NSApp postEvent:event atStart:YES];
        }
        [ck_state().wake clearQueued];
        return discarded;
    }
}

void ck_win_test_drain_events(void) {
    @autoreleasepool {
        for (int i = 0; i < 1000; i++) {
            NSEvent *event = [NSApp nextEventMatchingMask:NSEventMaskAny
                                                untilDate:[NSDate distantPast]
                                                   inMode:NSDefaultRunLoopMode
                                                  dequeue:YES];
            if (event == nil) {
                break;
            }
            [NSApp sendEvent:event];
        }
        [NSApp updateWindows];
    }
}

void ck_win_test_set_scale(double scale) {
    atomic_store(&ck_scale_override, scale);
}

void ck_win_test_backing_changed(void) {
    @autoreleasepool {
        NSWindow *window = ck_window();
        NSNotification *notification =
            [NSNotification notificationWithName:NSWindowDidChangeBackingPropertiesNotification
                                          object:window];
        [window.delegate windowDidChangeBackingProperties:notification];
    }
}

void ck_win_test_deminiaturized(void) {
    @autoreleasepool {
        NSWindow *window = ck_window();
        NSNotification *notification =
            [NSNotification notificationWithName:NSWindowDidDeminiaturizeNotification object:window];
        [window.delegate windowDidDeminiaturize:notification];
    }
}

void ck_win_test_miniaturized(void) {
    @autoreleasepool {
        NSWindow *window = ck_window();
        NSNotification *notification =
            [NSNotification notificationWithName:NSWindowDidMiniaturizeNotification object:window];
        [window.delegate windowDidMiniaturize:notification];
    }
}

void ck_win_test_burst(uint32_t button_presses, uint32_t pixel_changes, uint32_t needs_display) {
    @autoreleasepool {
        CKWinState *state = ck_state();
        for (uint32_t i = 0; i < button_presses; i++) {
            [state recordClose:CK_WIN_CLOSE_BUTTON];
        }
        for (uint32_t i = 0; i < pixel_changes; i++) {
            /* Each change is to a different scale, so each is a real change of the pixel size. */
            atomic_store(&ck_scale_override, 1.0 + (double)(i % 7 + 1) / 4.0);
            [state applyScaleAndRecordPixelSize];
        }
        for (uint32_t i = 0; i < needs_display; i++) {
            [state recordNeedsDisplay];
        }
    }
}

int32_t ck_win_test_record_keys(uint32_t pairs, uint16_t key_code) {
    @autoreleasepool {
        CKWinState *state = ck_state();
        uint16_t probe = key_code;
        uint8_t key;
        const char *name;
        BOOL known = NO;
        for (NSUInteger i = 0; i < ck_win_key_table_count(); i++) {
            uint16_t code;
            if (ck_win_key_table_entry(i, &code, &key, &name) && code == probe) {
                known = YES;
            }
        }
        if (!known) {
            return 0;
        }
        for (uint32_t i = 0; i < pairs; i++) {
            [state recordKeyCode:key_code down:YES repeat:NO];
            [state recordKeyCode:key_code down:NO repeat:NO];
        }
        return 1;
    }
}

/* Posts a marker whose dispatch (`sendEvent:`, inside a pump) records input natively: `kind` 0 records `pairs` down/up
 * pairs of `key_code`, 1 a focus loss, 2 a focus gain. The record is made while the pump dispatches. */
void ck_win_test_post_recording_marker(int32_t kind, uint32_t pairs, uint16_t key_code) {
    @autoreleasepool {
        NSInteger what = (NSInteger)((uint64_t)(uint32_t)kind | ((uint64_t)pairs << 8) | ((uint64_t)key_code << 32));
        NSEvent *event = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
                                            location:NSZeroPoint
                                       modifierFlags:0
                                           timestamp:0
                                        windowNumber:0
                                             context:nil
                                             subtype:CK_TEST_SUBTYPE
                                               data1:CK_TEST_RECORD_MARKER
                                               data2:what];
        [NSApp postEvent:event atStart:NO];
    }
}

void ck_win_test_set_absorbed(uint32_t count) {
    ck_state().closeAbsorbed = count;
}

void ck_win_test_schedule_records(double delay_seconds, uint32_t timers, uint32_t calls, int32_t kind) {
    @autoreleasepool {
        CKWinState *state = ck_state();
        NSDate *due = [NSDate dateWithTimeIntervalSinceNow:delay_seconds];
        for (uint32_t t = 0; t < timers; t++) {
            NSTimer *timer = [[NSTimer alloc] initWithFireDate:due
                                                      interval:0
                                                       repeats:NO
                                                         block:^(NSTimer *fired) {
                (void)fired;
                /* Shows the callbacks ran inside the pump (which services the timers while it waits). */
                void *frames[64];
                int count = backtrace(frames, 64);
                char **symbols = backtrace_symbols(frames, count);
                for (int i = 0; i < count; i++) {
                    if (strstr(symbols[i], "ck_win_pump") != NULL) {
                        atomic_fetch_add(&ck_in_pump, 1);
                        break;
                    }
                }
                free(symbols);
                for (uint32_t c = 0; c < calls; c++) {
                    if (kind == 3) {
                        [state recordNeedsDisplay];
                    } else {
                        [state recordClose:CK_WIN_CLOSE_BUTTON];
                    }
                }
            }];
            [[NSRunLoop mainRunLoop] addTimer:timer forMode:NSDefaultRunLoopMode];
        }
    }
}

void ck_win_test_native_keys(CKWinNativeKeysInfo *out) {
    CKWinNativeKeys keys;
    memset(&keys, 0, sizeof keys);
    [ck_state() nativeKeys:&keys];
    memcpy(out->held, keys.held, sizeof keys.held);
    memcpy(out->lost_pressed, keys.lost_pressed, sizeof keys.lost_pressed);
    memcpy(out->lost_released, keys.lost_released, sizeof keys.lost_released);
    out->ring_length = keys.ring_length;
    out->overflow = keys.overflow;
    out->focus_lost_pending = keys.focus_lost_pending;
    out->focus_gained_pending = keys.focus_gained_pending;
    out->overflow_total = keys.overflow_total;
}

uint32_t ck_win_test_key_table_count(void) {
    return (uint32_t)ck_win_key_table_count();
}

int32_t ck_win_test_key_table(uint32_t position, uint16_t *key_code, uint8_t *key, char *name,
                              uint32_t name_capacity) {
    const char *text;
    if (!ck_win_key_table_entry(position, key_code, key, &text)) {
        return 0;
    }
    snprintf(name, name_capacity, "%s", text);
    return 1;
}

void ck_win_test_resign_key(void) {
    @autoreleasepool {
        [[NSNotificationCenter defaultCenter] postNotificationName:NSWindowDidResignKeyNotification
                                                            object:ck_window()
                                                          userInfo:@{CK_SYNTHETIC_FOCUS_NOTIFICATION: @YES}];
    }
}

void ck_win_test_become_key(void) {
    @autoreleasepool {
        [[NSNotificationCenter defaultCenter] postNotificationName:NSWindowDidBecomeKeyNotification
                                                            object:ck_window()
                                                          userInfo:@{CK_SYNTHETIC_FOCUS_NOTIFICATION: @YES}];
    }
}

void ck_win_test_violate_main_thread(void) {
    @autoreleasepool {
        /* A classic Main Thread Checker report: marking a view dirty off the main thread. */
        [ck_window().contentView setNeedsDisplay:YES];
    }
}

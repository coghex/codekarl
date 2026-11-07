#import <AppKit/AppKit.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <unistd.h>
#define CK_APP_T_TIMEOUT (-3)
#define CK_APP_T_NO_WINDOW (-1)
#define CK_APP_T_SEVERAL_WINDOWS (-2)
extern void glfwPostEmptyEvent(void);
#include "../../../../crates/window-cocoa/tests/native/activation_watch.inc"

/* Test support for the application's windowed tests (tests/windowed.rs and tests/vulkan_app.rs), built into an archive
 * that only the package's integration tests link (build.rs). None of it is part of the library or the binary. Main
 * thread unless it says otherwise. */

/* The mark the tests' own synthetic keyboard events carry in kCGEventSourceUserData (real input carries 0), as in
 * ck-window-cocoa's test support. */
#define CK_TEST_INPUT_MARK 0x434B54455354LL
#define CK_TEST_POINTER_EVENT 0x434B50

static id ck_app_input_filter = nil;
static id ck_app_mouse_filter = nil;
static id ck_app_map_keys_observer = nil;
static id ck_app_sim_keys_observer = nil;
static id ck_app_key_files_observer = nil;
static dispatch_source_t ck_app_key_files_timer = nil;
static id ck_app_map_pointer_observer = nil;
static id ck_app_unmarked_observer = nil;
static _Atomic uint64_t ck_app_dropped;
static _Atomic uint64_t ck_app_dropped_mouse;
static NSWindow *ck_app_the_window(int32_t *error);
static BOOL ck_app_marker(const char *dir, const char *name);
void ck_app_t_schedule_map_keys(void);
void ck_app_t_schedule_sim_keys(void);
void ck_app_t_schedule_key_files(void);
void ck_app_t_schedule_map_pointer(void);
void ck_app_t_schedule_map_hover(void);

/* The test-only input filter: a local event monitor, which AppKit runs inside -[NSApplication sendEvent:] before
 * the event reaches a window, its key equivalents, a responder or text dispatch. It drops every keyboard event
 * (key down, key up, modifier change) that does not carry the tests' mark, so the owner's typing, Escape
 * included, reaches no test window, and passes the marked events the tests post on purpose. It is local to this
 * process. */
void ck_app_t_install_input_filter(void) {
    @autoreleasepool {
        if (ck_app_input_filter != nil) {
            return;
        }
        NSEventMask keyboard = NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged;
        ck_app_input_filter = [NSEvent addLocalMonitorForEventsMatchingMask:keyboard
                                                                    handler:^NSEvent *(NSEvent *event) {
            CGEventRef raw = event.CGEvent;
            if (raw != NULL &&
                CGEventGetIntegerValueField(raw, kCGEventSourceUserData) == CK_TEST_INPUT_MARK) {
                int32_t error = CK_APP_T_NO_WINDOW;
                NSWindow *window = ck_app_the_window(&error);
                if (window != nil && !window.keyWindow) {
                    [window sendEvent:event];
                    return nil;
                }
                return event;
            }
            atomic_fetch_add(&ck_app_dropped, 1);
            return nil;
        }];
        NSEventMask pointer = NSEventMaskLeftMouseDown | NSEventMaskLeftMouseDragged |
            NSEventMaskLeftMouseUp | NSEventMaskMouseMoved | NSEventMaskScrollWheel;
        ck_app_mouse_filter = [NSEvent addLocalMonitorForEventsMatchingMask:pointer
                                                                      handler:^NSEvent *(NSEvent *event) {
            CGEventRef raw = event.CGEvent;
            BOOL marked = event.eventNumber == CK_TEST_POINTER_EVENT ||
                (raw != NULL && CGEventGetIntegerValueField(raw, kCGEventSourceUserData) == CK_TEST_INPUT_MARK);
            if (marked) {
                int32_t error = CK_APP_T_NO_WINDOW;
                NSWindow *window = ck_app_the_window(&error);
                if (window != nil && !window.keyWindow) {
                    [window sendEvent:event];
                    return nil;
                }
                return event;
            }
            atomic_fetch_add(&ck_app_dropped_mouse, 1);
            return nil;
        }];
        if (getenv("CK_WINDOWED_TEST_MAP_INPUT") != NULL || getenv("CK_VULKAN_TEST_MAP_INPUT") != NULL) {
            ck_app_t_schedule_map_pointer();
        }
        if (getenv("CK_WINDOWED_TEST_MAP_HOVER") != NULL || getenv("CK_VULKAN_TEST_MAP_HOVER") != NULL) {
            ck_app_t_schedule_map_hover();
        }
        if (getenv("CK_WINDOWED_TEST_SIM_INPUT") != NULL || getenv("CK_VULKAN_TEST_SIM_INPUT") != NULL) {
            ck_app_t_schedule_sim_keys();
        }
        if (getenv("CK_WINDOWED_TEST_KEY_FILES") != NULL || getenv("CK_VULKAN_TEST_KEY_FILES") != NULL) {
            ck_app_t_schedule_key_files();
        }
    }
}

uint64_t ck_app_t_dropped_keys(void) {
    return atomic_load(&ck_app_dropped);
}

/* Makes a keyboard NSEvent from a CGEvent. kind 0 is a key-down, 1 a key-up, 2 a modifier change. */
static NSEvent *ck_app_key(int kind, uint16_t code, uint64_t flags, int repeat, const char *characters,
                           int marked) {
    CGEventRef raw = CGEventCreateKeyboardEvent(NULL, code, kind == 0);
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

/* Posts one key-down to the application: marked, the filter lets it through (a later windowed case presses a key
 * on purpose); unmarked, it is the owner's typing and the filter drops it. Returns 1 if the event was posted. */
int32_t ck_app_t_post_key(uint16_t key_code, uint64_t modifier_flags, int32_t repeat, const char *characters,
                          int32_t marked) {
    @autoreleasepool {
        NSEvent *event = ck_app_key(0, key_code, modifier_flags, repeat, characters, marked);
        if (event == nil) {
            return 0;
        }
        [NSApp postEvent:event atStart:NO];
        return 1;
    }
}

/* Posts the map-window test's marked pan and zoom keys only after AppKit reports a visible window. */
void ck_app_t_schedule_map_keys(void) {
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    ck_app_map_keys_observer = [center addObserverForName:NSWindowDidUpdateNotification
                                   object:nil
                                    queue:NSOperationQueue.mainQueue
                               usingBlock:^(__unused NSNotification *notification) {
        NSWindow *window = notification.object;
        if (![window isKindOfClass:NSWindow.class] || !window.isVisible) {
            return;
        }
        [NSNotificationCenter.defaultCenter removeObserver:ck_app_map_keys_observer];
        ck_app_map_keys_observer = nil;
        (void)ck_app_t_post_key(0x7B, 0, 0, "", 1);  /* ArrowLeft */
        (void)ck_app_t_post_key(0x18, 0, 0, "=", 1); /* Equal */
    }];
}

/* Calls the production content-view handlers with marked synthetic NSEvents. AppKit sendEvent cannot deliver pointer
 * events to this mouse-ignoring test window, so the manager-approved test-only seam skips only window routing. */
static int32_t ck_app_t_send_map_pointer(void) {
    int32_t error = CK_APP_T_NO_WINDOW;
    NSWindow *window = ck_app_the_window(&error);
    if (window == nil) return -1;
    if (!window.isVisible || !window.ignoresMouseEvents) return -2;
    NSView *view = window.contentView;

    const NSInteger marked_event = CK_TEST_POINTER_EVENT;
    const NSPoint drag_start = NSMakePoint(100.0, 80.0);
    const NSPoint drag_end = NSMakePoint(132.0, 96.0);
    NSEvent *down = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDown
                                        location:drag_start modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
                                   windowNumber:window.windowNumber context:nil eventNumber:marked_event
                                      clickCount:1 pressure:1.0];
    NSEvent *drag = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDragged
                                        location:drag_end modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
                                   windowNumber:window.windowNumber context:nil eventNumber:marked_event
                                      clickCount:1 pressure:1.0];
    NSEvent *up = [NSEvent mouseEventWithType:NSEventTypeLeftMouseUp
                                      location:drag_end modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
                                 windowNumber:window.windowNumber context:nil eventNumber:marked_event
                                    clickCount:1 pressure:0.0];
    if (down == nil || drag == nil || up == nil || down.windowNumber != window.windowNumber ||
        drag.windowNumber != window.windowNumber || up.windowNumber != window.windowNumber) return -3;
    @try {
        [view mouseDown:down];
        [view mouseDragged:drag];
        [view mouseUp:up];
    } @catch (NSException *exception) {
        NSLog(@"marked mouse dispatch raised %@: %@", exception.name, exception.reason);
        return -6;
    }

    NSPoint screen = [window convertPointToScreen:NSMakePoint(180.0, 120.0)];
    NSPoint scroll_point = NSMakePoint(screen.x, NSScreen.mainScreen.frame.size.height - screen.y);
    CGEventRef line_raw = CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitLine, 1, 1);
    CGEventRef precise_raw = CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitPixel, 1, 24);
    if (line_raw == NULL || precise_raw == NULL) {
        if (line_raw != NULL) CFRelease(line_raw);
        if (precise_raw != NULL) CFRelease(precise_raw);
        return -4;
    }
    CGEventRef scrolls[] = {line_raw, precise_raw};
    for (size_t i = 0; i < sizeof scrolls / sizeof scrolls[0]; i++) {
        CGEventSetLocation(scrolls[i], scroll_point);
        CGEventSetIntegerValueField(scrolls[i], kCGEventSourceUserData, CK_TEST_INPUT_MARK);
        CGEventSetIntegerValueField(scrolls[i], kCGMouseEventWindowUnderMousePointer, window.windowNumber);
    }
    CGEventSetIntegerValueField(precise_raw, kCGScrollWheelEventPointDeltaAxis1, 24);
    CGEventSetIntegerValueField(precise_raw, kCGScrollWheelEventIsContinuous, 1);
    NSEvent *line = [NSEvent eventWithCGEvent:line_raw];
    NSEvent *precise = [NSEvent eventWithCGEvent:precise_raw];
    CFRelease(line_raw);
    CFRelease(precise_raw);
    if (line == nil || precise == nil) return -9;
    @try {
        [view scrollWheel:line];
        [view scrollWheel:precise];
    } @catch (NSException *exception) {
        NSLog(@"marked scroll dispatch raised %@: %@", exception.name, exception.reason);
        return -7;
    }
    return window.ignoresMouseEvents ? 1 : -8;
}

void ck_app_t_schedule_map_pointer(void) {
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    ck_app_map_pointer_observer = [center addObserverForName:NSWindowDidUpdateNotification
                                   object:nil
                                    queue:NSOperationQueue.mainQueue
                               usingBlock:^(__unused NSNotification *notification) {
        NSWindow *window = notification.object;
        if (![window isKindOfClass:NSWindow.class] || !window.isVisible) return;
        [NSNotificationCenter.defaultCenter removeObserver:ck_app_map_pointer_observer];
        ck_app_map_pointer_observer = nil;
        NSEvent *unmarked = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDown
                                                location:NSMakePoint(150.0, 100.0)
                                           modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
                                          windowNumber:window.windowNumber context:nil eventNumber:0
                                             clickCount:1 pressure:1.0];
        if (unmarked != nil) [NSApp postEvent:unmarked atStart:NO];
        int32_t result = ck_app_t_send_map_pointer();
        if (result != 1) {
            NSLog(@"test pointer delivery failed with result %d", result);
        }
        ck_app_unmarked_observer = [NSNotificationCenter.defaultCenter addObserverForName:NSWindowDidUpdateNotification
                                        object:nil
                                         queue:NSOperationQueue.mainQueue
                                    usingBlock:^(__unused NSNotification *update) {
            if (atomic_load(&ck_app_dropped_mouse) < 1) return;
            [NSNotificationCenter.defaultCenter removeObserver:ck_app_unmarked_observer];
            ck_app_unmarked_observer = nil;
            const char *path = getenv("CK_MAP_TEST_CAMERA_OBSERVATION");
            if (path != NULL) {
                FILE *file = fopen(path, "a");
                if (file != NULL) {
                    fprintf(file, "unmarked dropped %llu\n", (unsigned long long)atomic_load(&ck_app_dropped_mouse));
                    fclose(file);
                }
            }
        }];
    }];
}

/* Zooms the fixture map around its centered place, hovers it, then moves away on later window updates. */
static int32_t ck_app_t_send_hover_event(NSWindow *window, int32_t kind, double x, double y) {
    if (!window.isVisible || !window.ignoresMouseEvents) return -1;
    NSView *view = window.contentView;
    const NSInteger marked_event = CK_TEST_POINTER_EVENT;
    if (kind == 3) {
        BOOL glfw_vulkan = getenv("CK_VULKAN_TEST_MAP_HOVER") != NULL;
        NSPoint screen = [window convertPointToScreen:NSMakePoint(x, y)];
        NSPoint screen_top = NSMakePoint(screen.x, NSScreen.mainScreen.frame.size.height - screen.y);
        CGScrollEventUnit unit = glfw_vulkan ? kCGScrollEventUnitLine : kCGScrollEventUnitPixel;
        CGEventRef raw = CGEventCreateScrollWheelEvent(NULL, unit, 1, 15);
        if (raw == NULL) return -2;
        CGEventSetLocation(raw, screen_top);
        if (!glfw_vulkan) {
            CGEventSetIntegerValueField(raw, kCGScrollWheelEventPointDeltaAxis1, 15);
            CGEventSetIntegerValueField(raw, kCGScrollWheelEventIsContinuous, 1);
        }
        CGEventSetIntegerValueField(raw, kCGEventSourceUserData, CK_TEST_INPUT_MARK);
        NSEvent *event = [NSEvent eventWithCGEvent:raw];
        CFRelease(raw);
        if (event == nil) return -3;
        [view scrollWheel:event];
        return 1;
    }
    NSEvent *event = [NSEvent mouseEventWithType:NSEventTypeMouseMoved
                                         location:NSMakePoint(x, y)
                                    modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
                                   windowNumber:window.windowNumber context:nil eventNumber:marked_event
                                      clickCount:0 pressure:0.0];
    if (event == nil || event.windowNumber != window.windowNumber) return -4;
    [view mouseMoved:event];
    return 1;
}

void ck_app_t_schedule_map_hover(void) {
    __block NSUInteger stage = 0;
    const char *markers = getenv("CK_WINDOWED_TEST_MAP_HOVER");
    if (markers == NULL) markers = getenv("CK_VULKAN_TEST_MAP_HOVER");
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    ck_app_map_pointer_observer = [center addObserverForName:NSWindowDidUpdateNotification
                                   object:nil
                                    queue:NSOperationQueue.mainQueue
                               usingBlock:^(__unused NSNotification *notification) {
        NSWindow *window = notification.object;
        if (![window isKindOfClass:NSWindow.class] || !window.isVisible) return;
        int32_t result = 1;
        if (stage == 0) {
            result = ck_app_t_send_hover_event(window, 3, 200.0, 120.0);
            if (result == 1) result = ck_app_t_send_hover_event(window, 1, 200.0, 120.0);
            stage = 1;
        } else if (stage == 1 && markers != NULL && ck_app_marker(markers, "hover-received")) {
            result = ck_app_t_send_hover_event(window, 1, 8.0, 8.0);
            [NSNotificationCenter.defaultCenter removeObserver:ck_app_map_pointer_observer];
            ck_app_map_pointer_observer = nil;
            stage = 2;
        }
        if (result != 1) NSLog(@"test hover dispatch failed with result %d", result);
    }];
}

/* What the typist's helper thread saw of its own signal mask, recorded from inside the GCD task: 0 until it
 * ran, then 1 plus 2 if SIGINT was blocked and 4 if SIGTERM was blocked. */
static _Atomic int32_t ck_app_typist_mask;

int32_t ck_app_t_typist_mask(void) {
    return atomic_load(&ck_app_typist_mask);
}

/* Posts a marked tap (key-down, then key-up) of one key. */
static void ck_app_tap(uint16_t code, const char *characters) {
    NSEvent *down = ck_app_key(0, code, 0, 0, characters, 1);
    NSEvent *up = ck_app_key(1, code, 0, 0, NULL, 1);
    if (down != nil) {
        [NSApp postEvent:down atStart:NO];
    }
    if (up != nil) {
        [NSApp postEvent:up atStart:NO];
    }
}

/* Posts a marked key-down, or a marked key-up, of one key. */
static void ck_app_hold(uint16_t code, const char *characters, int down) {
    NSEvent *event = ck_app_key(down ? 0 : 1, code, 0, 0, down ? characters : NULL, 1);
    if (event != nil) {
        [NSApp postEvent:event atStart:NO];
    }
}

/* Whether `name` exists in the directory `dir`. */
static BOOL ck_app_marker(const char *dir, const char *name) {
    NSString *path = [[NSString stringWithUTF8String:dir] stringByAppendingPathComponent:
                      [NSString stringWithUTF8String:name]];
    return access(path.fileSystemRepresentation, F_OK) == 0;
}

/* Posts the simulation window test's marked keys in three batches, each when the test says so: Space and Digit5 (resume,
 * then speed 5) as soon as AppKit reports a visible window; a map pan (ArrowLeft, held down) once the directory named by
 * the environment holds a file `pan`; their key-ups, Space and F5 (pause, then quicksave) once it holds `pause`. The test writes each marker
 * after it has seen the simulation's records show the game progress, so the batches are ordered by what happened, not by a
 * count or a delay. The poll runs on the main queue every 5 ms and ends with the last batch. */
void ck_app_t_schedule_sim_keys(void) {
    const char *dir = getenv("CK_WINDOWED_TEST_SIM_INPUT");
    if (dir == NULL) {
        dir = getenv("CK_VULKAN_TEST_SIM_INPUT");
    }
    if (dir == NULL) {
        return;
    }
    static char markers[1024];
    strncpy(markers, dir, sizeof markers - 1);
    ck_app_sim_keys_observer = [NSNotificationCenter.defaultCenter
        addObserverForName:NSWindowDidUpdateNotification
                    object:nil
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification *notification) {
        NSWindow *window = notification.object;
        if (![window isKindOfClass:NSWindow.class] || !window.isVisible) {
            return;
        }
        [NSNotificationCenter.defaultCenter removeObserver:ck_app_sim_keys_observer];
        ck_app_sim_keys_observer = nil;
        ck_app_tap(0x31, " "); /* Space */
        ck_app_tap(0x17, "5"); /* Digit5 */
        __block BOOL panned = NO;
        dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
        dispatch_source_set_timer(timer, DISPATCH_TIME_NOW, 5 * NSEC_PER_MSEC, NSEC_PER_MSEC);
        dispatch_source_set_event_handler(timer, ^{
            if (!panned && ck_app_marker(markers, "pan")) {
                panned = YES;
                ck_app_hold(0x7B, "", 1); /* ArrowLeft down: a pan held while the game runs */
            }
            if (ck_app_marker(markers, "pause")) {
                ck_app_hold(0x7B, "", 0); /* ArrowLeft up */
                ck_app_tap(0x31, " "); /* Space */
                ck_app_tap(0x60, "");  /* F5 */
                dispatch_source_cancel(timer);
            }
        });
        dispatch_resume(timer);
    }];
}

/* Posts the marked keys the test asks for, one at a time, once AppKit reports a visible window. The environment names a directory;
 * the test writes a file `NNN.key` there for each key it wants pressed, holding `<hexadecimal key code>:<characters>`, and the
 * poll (on the main queue, every 5 ms) taps each such file's key (a key-down and a key-up, marked) in name order, then renames
 * the file to `NNN.key.done`. So the keys are ordered by what the test has seen happen, not by a count or a delay. */
void ck_app_t_schedule_key_files(void) {
    const char *dir = getenv("CK_WINDOWED_TEST_KEY_FILES");
    if (dir == NULL) {
        dir = getenv("CK_VULKAN_TEST_KEY_FILES");
    }
    if (dir == NULL) {
        return;
    }
    static char directory[1024];
    strncpy(directory, dir, sizeof directory - 1);
    ck_app_key_files_observer = [NSNotificationCenter.defaultCenter
        addObserverForName:NSWindowDidUpdateNotification
                    object:nil
                     queue:NSOperationQueue.mainQueue
                usingBlock:^(NSNotification *notification) {
        NSWindow *window = notification.object;
        if (![window isKindOfClass:NSWindow.class] || !window.isVisible) {
            return;
        }
        [NSNotificationCenter.defaultCenter removeObserver:ck_app_key_files_observer];
        ck_app_key_files_observer = nil;
        /* Kept in a static: nothing else holds the source, and a released timer never fires. */
        ck_app_key_files_timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
        dispatch_source_set_timer(ck_app_key_files_timer, DISPATCH_TIME_NOW, 5 * NSEC_PER_MSEC, NSEC_PER_MSEC);
        dispatch_source_set_event_handler(ck_app_key_files_timer, ^{
            NSString *root = [NSString stringWithUTF8String:directory];
            NSArray<NSString *> *names = [[NSFileManager.defaultManager contentsOfDirectoryAtPath:root error:nil]
                                          sortedArrayUsingSelector:@selector(compare:)];
            for (NSString *name in names) {
                if (![name hasSuffix:@".key"]) {
                    continue;
                }
                NSString *path = [root stringByAppendingPathComponent:name];
                NSString *text = [NSString stringWithContentsOfFile:path encoding:NSUTF8StringEncoding error:nil];
                NSRange colon = [text rangeOfString:@":"];
                if (colon.location == NSNotFound) {
                    continue;
                }
                unsigned code = 0;
                if ([[NSScanner scannerWithString:[text substringToIndex:colon.location]] scanHexInt:&code]) {
                    ck_app_tap((uint16_t)code, [text substringFromIndex:colon.location + 1].UTF8String);
                }
                [NSFileManager.defaultManager moveItemAtPath:path
                                                      toPath:[path stringByAppendingString:@".done"]
                                                       error:nil];
            }
        });
        dispatch_resume(ck_app_key_files_timer);
    }];
}

/* The owner, typing without stopping: unmarked key-downs (Escape among them), a repeat, a key-up, modifier
 * changes and a Command-Q, posted to the application every 5 ms. The first thing the helper thread does is
 * report its signal mask. */
void ck_app_t_start_typist(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            sigset_t mask;
            pthread_sigmask(SIG_BLOCK, NULL, &mask);
            int32_t seen = 1;
            if (sigismember(&mask, SIGINT) == 1) {
                seen |= 2;
            }
            if (sigismember(&mask, SIGTERM) == 1) {
                seen |= 4;
            }
            atomic_store(&ck_app_typist_mask, seen);
            for (;;) {
                @autoreleasepool {
                    NSEvent *events[] = {
                        ck_app_key(0, 53, 0, 0, "\x1b", 0),
                        ck_app_key(0, 53, 0, 1, "\x1b", 0),
                        ck_app_key(1, 53, 0, 0, NULL, 0),
                        ck_app_key(0, 0, 0, 0, "a", 0),
                        ck_app_key(2, 55, NSEventModifierFlagCommand, 0, NULL, 0),
                        ck_app_key(0, 12, NSEventModifierFlagCommand, 0, "q", 0),
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

/* ---- The window helpers of tests/vulkan_app.rs (the GLFW window of a glfw-vulkan run) ----------------------------- */

/* Runs `work` on the main queue and returns its result; callable from any thread. The caller waits at most one
 * second: a main queue that does not run the block gives CK_APP_T_TIMEOUT instead of a hang or a late answer. */
static int32_t ck_app_on_main(int32_t (^work)(void)) {
    if ([NSThread isMainThread]) {
        return work();
    }
    __block int32_t result = CK_APP_T_TIMEOUT;
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_main_queue(), ^{
        result = work();
        dispatch_semaphore_signal(done);
    });
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 1 * NSEC_PER_SEC)) != 0) {
        return CK_APP_T_TIMEOUT;
    }
    return result;
}

/* The process's one Codekarl window (Cocoa or GLFW), or nil with *error set when none or several exist. Main queue only. */
static NSWindow *ck_app_the_window(int32_t *error) {
    Class glfw = NSClassFromString(@"GLFWWindow");
    Class cocoa_view = NSClassFromString(@"CKWinView");
    NSWindow *found = nil;
    for (NSWindow *window in [NSApp windows]) {
        BOOL is_codekarl_window = (glfw != Nil && [window isKindOfClass:glfw]) ||
            (cocoa_view != Nil && [window.contentView isKindOfClass:cocoa_view]);
        if (is_codekarl_window) {
            if (found != nil) {
                *error = CK_APP_T_SEVERAL_WINDOWS;
                return nil;
            }
            found = window;
        }
    }
    if (found == nil) {
        *error = CK_APP_T_NO_WINDOW;
    }
    return found;
}

/* Asks the one GLFW window to miniaturize. Returns 1 once asked, or a negative error. The request is not an
 * acknowledgement: the test reads the window's state (ck_app_t_window_state) until it says so. */
int32_t ck_app_t_miniaturize(void) {
    return ck_app_on_main(^int32_t {
        int32_t error = 0;
        NSWindow *window = ck_app_the_window(&error);
        if (window == nil) {
            return error;
        }
        [window miniaturize:nil];
        return 1;
    });
}

/* Asks the one GLFW window to deminiaturize. Returns 1 once asked, or a negative error. */
int32_t ck_app_t_deminiaturize(void) {
    return ck_app_on_main(^int32_t {
        int32_t error = 0;
        NSWindow *window = ck_app_the_window(&error);
        if (window == nil) {
            return error;
        }
        [window deminiaturize:nil];
        return 1;
    });
}

/* Wakes the GLFW event pump after the driver has observed the native window restored. */
int32_t ck_app_t_wake_pump(void) {
    glfwPostEmptyEvent();
    return 1;
}

/* The one GLFW window's actual state, read on the main queue: bit 0 is isMiniaturized, bit 1 is isVisible. A
 * negative value is an error (none, several windows, or a main queue that did not run the read in time). */
int32_t ck_app_t_window_state(void) {
    return ck_app_on_main(^int32_t {
        int32_t error = 0;
        NSWindow *window = ck_app_the_window(&error);
        if (window == nil) {
            return error;
        }
        return (window.isMiniaturized ? 1 : 0) | (window.isVisible ? 2 : 0);
    });
}

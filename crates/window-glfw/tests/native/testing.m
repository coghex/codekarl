#import <Cocoa/Cocoa.h>
#import <objc/runtime.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include "testing.h"
#include "../../../window-cocoa/tests/native/activation_watch.inc"

/* The mark the tests' synthetic keyboard events carry in kCGEventSourceUserData (real input carries 0). */
#define CK_TEST_INPUT_MARK 0x434B54455354LL

static _Atomic uint64_t ck_filtered_keys;
static _Atomic double ck_scale_override;
static _Atomic int32_t ck_policy_refuse;
static _Atomic uint64_t ck_policy_calls;
static _Atomic uint64_t ck_activate_calls;

/* The backend's windows. Once the application is active AppKit adds windows of its own to NSApp.windows
 * (the text input UI's TUINSWindow, for one), which are not the backend's and come and go. */
static NSArray<NSWindow *> *ck_glfw_windows(void) {
    Class glfw_window = NSClassFromString(@"GLFWWindow");
    NSMutableArray<NSWindow *> *found = [NSMutableArray array];
    for (NSWindow *window in NSApp.windows) {
        if (glfw_window != Nil && [window isKindOfClass:glfw_window]) {
            [found addObject:window];
        }
    }
    return found;
}

static NSWindow *ck_window(void) {
    return ck_glfw_windows().firstObject;
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

void ck_glfw_test_info(CKGlfwTestInfo *out) {
    @autoreleasepool {
        memset(out, 0, sizeof *out);
        /* Messaging a nil NSApp reads as the regular policy (0), so say plainly that there is none. */
        out->activation_policy = NSApp != nil ? (int32_t)NSApp.activationPolicy : -1;
        out->is_active = (NSApp != nil && NSApp.active) ? 1 : 0;
        out->has_main_menu = NSApp.mainMenu != nil ? 1 : 0;
        if (NSApp.mainMenu != nil) {
            ck_count_menu(NSApp.mainMenu, &out->menu_items, &out->menu_key_equivalents);
        }
        out->window_count = (uint64_t)ck_glfw_windows().count;
        out->filtered_keys = atomic_load(&ck_filtered_keys);
        NSWindow *window = ck_window();
        if (window == nil) {
            return;
        }
        out->has_window = 1;
        snprintf(out->title, sizeof out->title, "%s", window.title.UTF8String ?: "");
        NSRect content = [window contentRectForFrameRect:window.frame];
        out->content_width = content.size.width;
        out->content_height = content.size.height;
        out->resizable = (window.styleMask & NSWindowStyleMaskResizable) != 0;
        out->visible = window.visible ? 1 : 0;
        out->is_key = window.keyWindow ? 1 : 0;
        out->window_scale = window.backingScaleFactor;
    }
}

size_t ck_glfw_test_info_layout(uint64_t *out, size_t capacity) {
    const uint64_t values[] = {
        (uint64_t)sizeof(CKGlfwTestInfo),
        (uint64_t)_Alignof(CKGlfwTestInfo),
#define CK_OFFSET(field) (uint64_t)offsetof(CKGlfwTestInfo, field),
        CK_GLFW_TEST_INFO_FIELDS(CK_OFFSET)
#undef CK_OFFSET
    };
    const size_t count = sizeof values / sizeof values[0];
    for (size_t i = 0; i < count && i < capacity; i++) {
        out[i] = values[i];
    }
    return count;
}

void ck_glfw_test_activate(void) {
    @autoreleasepool {
        [NSApp activateIgnoringOtherApps:YES];
        [ck_window() makeKeyAndOrderFront:nil];
    }
}

int64_t ck_glfw_test_window_number(void) {
    @autoreleasepool {
        return (int64_t)ck_window().windowNumber;
    }
}

void ck_glfw_test_post_mouse_moved(int64_t window_number) {
    @autoreleasepool {
        NSEvent *event = [NSEvent mouseEventWithType:NSEventTypeMouseMoved
                                            location:NSMakePoint(20, 20)
                                       modifierFlags:0
                                           timestamp:[NSProcessInfo processInfo].systemUptime
                                        windowNumber:(NSInteger)window_number
                                             context:nil
                                         eventNumber:0
                                          clickCount:0
                                            pressure:0];
        if (event != nil) {
            [NSApp postEvent:event atStart:NO];
        }
    }
}

void ck_glfw_test_perform_close(void) {
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

int32_t ck_glfw_test_post_key(uint16_t key_code, uint64_t modifier_flags, int32_t repeat,
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

int32_t ck_glfw_test_post_keyboard_event(int32_t kind, uint16_t key_code, uint64_t modifier_flags,
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
 * sendEvent:] before the event reaches a window, its key equivalents or keyDown:, and so before GLFW sees
 * it. It drops every keyboard event (key down, key up, modifier change) that does not carry the tests'
 * mark, so the owner's typing, Escape included, never reaches a test window. */
static id ck_input_filter = nil;
void ck_glfw_test_install_input_filter(void) {
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
                NSWindow *window = ck_window();
                if (window != nil && !window.keyWindow) {
                    [window sendEvent:event];
                    return nil;
                }
                return event;
            }
            atomic_fetch_add(&ck_filtered_keys, 1);
            return nil;
        }];
    }
}

/* The owner, typing without stopping: unmarked events of every keyboard kind, Escape among them. */
void ck_glfw_test_start_typist(void) {
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
 * The activation policy
 * ------------------------------------------------------------------------------------------- */

void ck_glfw_test_watch_policy(int32_t refuse) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        Method method = class_getInstanceMethod([NSApplication class], @selector(setActivationPolicy:));
        BOOL (*original)(id, SEL, NSApplicationActivationPolicy) =
            (BOOL(*)(id, SEL, NSApplicationActivationPolicy))method_getImplementation(method);
        method_setImplementation(method, imp_implementationWithBlock(
            ^BOOL(NSApplication *application, NSApplicationActivationPolicy policy) {
                atomic_fetch_add(&ck_policy_calls, 1);
                if (atomic_load(&ck_policy_refuse) != 0) {
                    return NO;
                }
                return original(application, @selector(setActivationPolicy:), policy);
            }));
    });
    atomic_store(&ck_policy_refuse, refuse);
}

uint64_t ck_glfw_test_policy_calls(void) {
    return atomic_load(&ck_policy_calls);
}

int32_t ck_glfw_test_watch_activate(void) {
    SEL selector = NSSelectorFromString(@"activate");
    Method method = class_getInstanceMethod([NSApplication class], selector);
    if (method == NULL) {
        return 0;
    }
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        void (*original)(id, SEL) = (void (*)(id, SEL))method_getImplementation(method);
        method_setImplementation(method, imp_implementationWithBlock(^void(NSApplication *application) {
            atomic_fetch_add(&ck_activate_calls, 1);
            original(application, selector);
        }));
    });
    return 1;
}

uint64_t ck_glfw_test_activate_calls(void) {
    return atomic_load(&ck_activate_calls);
}

/* ---------------------------------------------------------------------------------------------
 * The event trace
 * ------------------------------------------------------------------------------------------- */

static NSMutableArray<NSString *> *ck_trace_lines;
static _Atomic uint64_t ck_trace_origin;

static void ck_trace_add(NSString *what) {
    double ms = (double)(clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - atomic_load(&ck_trace_origin)) / 1e6;
    NSString *line = [NSString stringWithFormat:@"%9.1f ms  %@", ms, what];
    @synchronized(ck_trace_lines) {
        [ck_trace_lines addObject:line];
    }
}

void ck_glfw_test_trace_start(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        ck_trace_lines = [NSMutableArray array];
        [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskAny
                                              handler:^NSEvent *(NSEvent *event) {
            /* -[NSEvent subtype] raises for the types that have none. */
            BOOL has_subtype = event.type == NSEventTypeApplicationDefined ||
                               event.type == NSEventTypeSystemDefined ||
                               event.type == NSEventTypeAppKitDefined;
            ck_trace_add([NSString stringWithFormat:@"event type=%ld subtype=%ld window=%@",
                          (long)event.type, has_subtype ? (long)event.subtype : -1L,
                          event.window != nil ? @"yes" : @"no"]);
            return event;
        }];
        [[NSNotificationCenter defaultCenter]
            addObserverForName:nil
                        object:nil
                         queue:nil
                    usingBlock:^(NSNotification *notification) {
            ck_trace_add([NSString stringWithFormat:@"notification %@", notification.name]);
        }];
    });
    atomic_store(&ck_trace_origin, clock_gettime_nsec_np(CLOCK_UPTIME_RAW));
}

void ck_glfw_test_trace_mark(const char *text) {
    if (ck_trace_lines != nil) {
        ck_trace_add([NSString stringWithFormat:@"==== %s", text]);
    }
}

size_t ck_glfw_test_trace_take(char *out, size_t capacity) {
    if (capacity == 0) {
        return 0;
    }
    NSString *text = @"";
    @synchronized(ck_trace_lines) {
        text = [ck_trace_lines componentsJoinedByString:@"\n"];
        [ck_trace_lines removeAllObjects];
    }
    snprintf(out, capacity, "%s", text.UTF8String ?: "");
    return strlen(out);
}

/* ---------------------------------------------------------------------------------------------
 * The scale factor
 * ------------------------------------------------------------------------------------------- */

static IMP ck_original_scale_factor;

void ck_glfw_test_set_scale(double scale) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        Method method = class_getInstanceMethod([NSWindow class], @selector(backingScaleFactor));
        ck_original_scale_factor = method_getImplementation(method);
        method_setImplementation(method, imp_implementationWithBlock(^CGFloat(NSWindow *window) {
            double override = atomic_load(&ck_scale_override);
            return override > 0 ? override
                                : ((CGFloat(*)(id, SEL))ck_original_scale_factor)(
                                      window, @selector(backingScaleFactor));
        }));
    });
    atomic_store(&ck_scale_override, scale);
}

void ck_glfw_test_backing_changed(void) {
    @autoreleasepool {
        NSWindow *window = ck_window();
        /* The notification AppKit posts, and the view method it drives, as a display change does. */
        [[NSNotificationCenter defaultCenter]
            postNotificationName:NSWindowDidChangeBackingPropertiesNotification
                          object:window];
        [window.contentView viewDidChangeBackingProperties];
    }
}

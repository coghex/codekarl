#import <AppKit/AppKit.h>
#include <stdatomic.h>
#include <stdint.h>
#include <unistd.h>

/* Test support for ck-render-2d's real-window test (tests/metal_window.rs), built into an archive that only
 * the test executable links (build.rs). None of it is part of the library. Main thread unless it says
 * otherwise. */

static id ck_r2_input_filter = nil;
static _Atomic uint64_t ck_r2_dropped;

/* The test-only input filter: a local event monitor, which AppKit runs inside -[NSApplication
 * sendEvent:] before the event reaches a window, its key equivalents or keyDown:. These tests post no
 * keyboard event of their own, so it drops every keyboard event (key down, key up, modifier change):
 * the owner's typing, Escape included, reaches no test window. It is local to this process. */
void ck_r2_t_install_input_filter(void) {
    @autoreleasepool {
        if (ck_r2_input_filter != nil) {
            return;
        }
        NSEventMask keyboard = NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged;
        ck_r2_input_filter = [NSEvent addLocalMonitorForEventsMatchingMask:keyboard
                                                                handler:^NSEvent *(NSEvent *event) {
            (void)event;
            atomic_fetch_add(&ck_r2_dropped, 1);
            return nil;
        }];
    }
}

uint64_t ck_r2_t_dropped_keys(void) {
    return atomic_load(&ck_r2_dropped);
}

/* The owner, typing without stopping: key-downs (Escape among them), a repeat, a key-up and modifier
 * changes, posted to the application every 5 ms. */
static NSEvent *ck_r2_key(int kind, uint16_t code, uint64_t flags, int repeat, const char *characters) {
    CGEventRef raw = CGEventCreateKeyboardEvent(NULL, code, kind == 0);
    if (raw == NULL) {
        return nil;
    }
    if (kind == 2) {
        CGEventSetType(raw, kCGEventFlagsChanged);
    }
    CGEventSetFlags(raw, (CGEventFlags)flags);
    CGEventSetIntegerValueField(raw, kCGKeyboardEventAutorepeat, repeat);
    if (characters != NULL) {
        NSString *chars = [NSString stringWithUTF8String:characters];
        UniChar units[4];
        NSUInteger length = MIN(chars.length, (NSUInteger)4);
        [chars getCharacters:units range:NSMakeRange(0, length)];
        CGEventKeyboardSetUnicodeString(raw, (UniCharCount)length, units);
    }
    NSEvent *event = [NSEvent eventWithCGEvent:raw];
    CFRelease(raw);
    return event;
}

void ck_r2_t_start_typist(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            for (;;) {
                @autoreleasepool {
                    NSEvent *events[] = {
                        ck_r2_key(0, 53, 0, 0, "\x1b"),
                        ck_r2_key(0, 53, 0, 1, "\x1b"),
                        ck_r2_key(1, 53, 0, 0, NULL),
                        ck_r2_key(0, 0, 0, 0, "a"),
                        ck_r2_key(2, 55, NSEventModifierFlagCommand, 0, NULL),
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


#import <AppKit/AppKit.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <pthread.h>
#include <unistd.h>

/* Test support for the application's windowed tests (tests/windowed.rs), built into an archive that only the
 * package's integration tests link (build.rs). None of it is part of the library or the binary. Main thread unless
 * it says otherwise. */

/* The mark the tests' own synthetic keyboard events carry in kCGEventSourceUserData (real input carries 0), as in
 * ck-window-cocoa's test support. */
#define CK_TEST_INPUT_MARK 0x434B54455354LL

static id ck_app_input_filter = nil;
static _Atomic uint64_t ck_app_dropped;

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
                return event;
            }
            atomic_fetch_add(&ck_app_dropped, 1);
            return nil;
        }];
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

/* What the typist's helper thread saw of its own signal mask, recorded from inside the GCD task: 0 until it
 * ran, then 1 plus 2 if SIGINT was blocked and 4 if SIGTERM was blocked. */
static _Atomic int32_t ck_app_typist_mask;

int32_t ck_app_t_typist_mask(void) {
    return atomic_load(&ck_app_typist_mask);
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

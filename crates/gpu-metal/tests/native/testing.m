#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdint.h>
#include <time.h>
#include <unistd.h>

/* Test support for the presenter's window tests, built into an archive that only the test executable
 * links. None of it is part of the library. Main thread unless it says otherwise. */

static id ck_input_filter = nil;
static _Atomic uint64_t ck_dropped;
/* Non-owning test identity token: compared only with contentView.layer, never dereferenced. */
static __unsafe_unretained CALayer *ck_rig_layer;
static NSDictionary *ck_minimise_target;
static NSDictionary *ck_last_poll_target;
static NSDictionary *ck_decoy_minimise_target;
static NSMutableArray<NSDictionary *> *ck_will_minimise;
static NSMutableArray<NSDictionary *> *ck_did_minimise;
static id ck_will_observer;
static id ck_did_observer;
static NSWindow *ck_minimise_decoy;

static uint64_t ck_uptime_raw_ns(void) {
    struct timespec now = {0};
    if (clock_gettime(CLOCK_UPTIME_RAW, &now) != 0) {
        return 0;
    }
    return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}

static NSDictionary *ck_window_target(NSWindow *window) {
    if (window == nil) {
        return @{@"window_number": [NSNull null], @"matches_rig_layer": @NO};
    }
    BOOL matches = ck_rig_layer != nil && window.contentView.layer == ck_rig_layer;
    return @{@"window_number": @(window.windowNumber), @"matches_rig_layer": @(matches)};
}

static NSArray<NSDictionary *> *ck_window_notifications(NSMutableArray<NSDictionary *> *rows) {
    @synchronized (rows) {
        return [rows copy];
    }
}

static id ck_window_number(NSWindow *window) {
    return window == nil ? [NSNull null] : @(window.windowNumber);
}

/* The selector is deliberately strict: callers may act only when exactly one content view hosts the rig layer. */
static NSWindow *ck_window_for_layer(NSArray<NSWindow *> *windows, CALayer *layer, NSUInteger *match_count) {
    NSWindow *match = nil;
    NSUInteger count = 0;
    for (NSWindow *window in windows) {
        if (layer != nil && window.contentView.layer == layer) {
            match = window;
            count++;
        }
    }
    if (match_count != NULL) {
        *match_count = count;
    }
    return count == 1 ? match : nil;
}

/* The test-only input filter: a local event monitor, which AppKit runs inside -[NSApplication
 * sendEvent:] before the event reaches a window, its key equivalents or keyDown:. These tests post no
 * keyboard event of their own, so it drops every keyboard event (key down, key up, modifier change):
 * the owner's typing, Escape included, reaches no test window. It is local to this process. */
void ck_t_install_input_filter(void) {
    @autoreleasepool {
        if (ck_input_filter != nil) {
            return;
        }
        ck_will_minimise = [NSMutableArray array];
        ck_did_minimise = [NSMutableArray array];
        NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
        ck_will_observer = [center addObserverForName:NSWindowWillMiniaturizeNotification
                                                object:nil
                                                 queue:nil
                                            usingBlock:^(NSNotification *notification) {
            NSWindow *window = notification.object;
            NSDictionary *row = @{@"window_number": ck_window_number(window),
                                  @"uptime_raw_ns": @(ck_uptime_raw_ns())};
            @synchronized (ck_will_minimise) {
                [ck_will_minimise addObject:row];
            }
        }];
        ck_did_observer = [center addObserverForName:NSWindowDidMiniaturizeNotification
                                               object:nil
                                                queue:nil
                                           usingBlock:^(NSNotification *notification) {
            NSWindow *window = notification.object;
            NSDictionary *row = @{@"window_number": ck_window_number(window),
                                  @"uptime_raw_ns": @(ck_uptime_raw_ns())};
            @synchronized (ck_did_minimise) {
                [ck_did_minimise addObject:row];
            }
        }];
        NSEventMask keyboard = NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged;
        ck_input_filter = [NSEvent addLocalMonitorForEventsMatchingMask:keyboard
                                                                handler:^NSEvent *(NSEvent *event) {
            (void)event;
            atomic_fetch_add(&ck_dropped, 1);
            return nil;
        }];
    }
}

void ck_t_set_rig_layer(void *layer) {
    @autoreleasepool {
        ck_rig_layer = (__bridge CALayer *)layer;
        ck_minimise_target = nil;
        ck_last_poll_target = nil;
    }
}

uint64_t ck_t_dropped_keys(void) {
    return atomic_load(&ck_dropped);
}

/* The owner, typing without stopping: key-downs (Escape among them), a repeat, a key-up and modifier
 * changes, posted to the application every 5 ms. */
static NSEvent *ck_key(int kind, uint16_t code, uint64_t flags, int repeat, const char *characters) {
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

void ck_t_start_typist(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            for (;;) {
                @autoreleasepool {
                    NSEvent *events[] = {
                        ck_key(0, 53, 0, 0, "\x1b"),
                        ck_key(0, 53, 0, 1, "\x1b"),
                        ck_key(1, 53, 0, 0, NULL),
                        ck_key(0, 0, 0, 0, "a"),
                        ck_key(2, 55, NSEventModifierFlagCommand, 0, NULL),
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

/* Minimises the application's window; reports whether it is miniaturized. */
int32_t ck_t_miniaturize(uint64_t *match_count) {
    @autoreleasepool {
        NSUInteger count = 0;
        NSWindow *target = ck_window_for_layer(NSApp.windows, ck_rig_layer, &count);
        if (match_count != NULL) {
            *match_count = (uint64_t)count;
        }
        if (target == nil) {
            ck_minimise_target = nil;
            return 0;
        }
        ck_minimise_target = ck_window_target(target);
        [target miniaturize:nil];
        return 1;
    }
}

int32_t ck_t_is_miniaturized(uint64_t *match_count) {
    @autoreleasepool {
        NSUInteger count = 0;
        NSWindow *target = ck_window_for_layer(NSApp.windows, ck_rig_layer, &count);
        if (match_count != NULL) {
            *match_count = (uint64_t)count;
        }
        if (target == nil) {
            ck_last_poll_target = nil;
            return -1;
        }
        ck_last_poll_target = ck_window_target(target);
        return target.miniaturized ? 1 : 0;
    }
}

/* Directly exercise the same selector with zero, one, and duplicate matching entries. */
int32_t ck_t_test_window_lookup(uint32_t mode, uint64_t *match_count) {
    @autoreleasepool {
        NSArray<NSWindow *> *windows = NSApp.windows;
        CALayer *layer = ck_rig_layer;
        if (mode == 2) {
            NSWindow *target = ck_window_for_layer(NSApp.windows, ck_rig_layer, NULL);
            if (target != nil) {
                windows = @[target, target];
            }
        } else if (mode == 0) {
            /* A fresh unhosted layer over real windows exercises the actual zero-match path. */
            layer = [CALayer layer];
        } else if (mode != 1) {
            return 0;
        }
        NSUInteger count = 0;
        NSWindow *target = ck_window_for_layer(windows, layer, &count);
        if (match_count != NULL) {
            *match_count = (uint64_t)count;
        }
        return target == nil ? 0 : 1;
    }
}

/* A deterministic wrong-target control. The decoy is explicitly passed to the old-style action; array order is irrelevant. */
int32_t ck_t_open_minimise_decoy(uint64_t *decoy_window_number,
                                 uint64_t *first_object_window_number,
                                 uint64_t *first_object_matches_rig) {
    @autoreleasepool {
        NSRect frame = NSMakeRect(0, 0, 40, 40);
        ck_minimise_decoy = [[NSWindow alloc] initWithContentRect:frame
                                                       styleMask:NSWindowStyleMaskBorderless
                                                         backing:NSBackingStoreBuffered
                                                           defer:NO];
        ck_minimise_decoy.releasedWhenClosed = NO;
        ck_minimise_decoy.animationBehavior = NSWindowAnimationBehaviorNone;
        [ck_minimise_decoy orderFrontRegardless];
        if (decoy_window_number != NULL) {
            *decoy_window_number = (uint64_t)ck_minimise_decoy.windowNumber;
        }
        NSWindow *first = NSApp.windows.firstObject;
        if (first_object_window_number != NULL) {
            *first_object_window_number = first == nil ? UINT64_MAX : (uint64_t)first.windowNumber;
        }
        if (first_object_matches_rig != NULL) {
            *first_object_matches_rig = (uint64_t)(ck_rig_layer != nil && first.contentView.layer == ck_rig_layer);
        }
        return ck_minimise_decoy != nil ? 1 : 0;
    }
}

void ck_t_minimise_decoy_explicitly(uint64_t *target_window_number,
                                   uint64_t *target_matches_rig_layer) {
    @autoreleasepool {
        ck_decoy_minimise_target = ck_window_target(ck_minimise_decoy);
        if (target_window_number != NULL) {
            *target_window_number = (uint64_t)ck_minimise_decoy.windowNumber;
        }
        if (target_matches_rig_layer != NULL) {
            *target_matches_rig_layer = [ck_decoy_minimise_target[@"matches_rig_layer"] boolValue];
        }
        [ck_minimise_decoy miniaturize:nil];
    }
}

int32_t ck_t_decoy_is_miniaturized(void) {
    @autoreleasepool {
        return ck_minimise_decoy.isMiniaturized ? 1 : 0;
    }
}

void ck_t_close_minimise_decoy(void) {
    @autoreleasepool {
        [ck_minimise_decoy close];
        ck_minimise_decoy = nil;
        ck_decoy_minimise_target = nil;
    }
}

/* The report-only post-failure observation asks about the rig's own window, separately from the old
 * firstObject helpers whose actual targets are being captured. */
int32_t ck_t_rig_is_miniaturized(void) {
    @autoreleasepool {
        for (NSWindow *window in NSApp.windows) {
            if (ck_rig_layer != nil && window.contentView.layer == ck_rig_layer) {
                return window.miniaturized ? 1 : 0;
            }
        }
        return 0;
    }
}

/* Emit one self-contained JSON diagnostic line and flush before the worker continues observing. */
void ck_t_print_minimise_diagnostics(const char *phase, uint64_t observed_ms,
                                    int32_t observed, int32_t truncated) {
    @autoreleasepool {
        NSString *phase_text = phase == NULL ? @"unknown" : [NSString stringWithUTF8String:phase];
        NSMutableArray<NSDictionary *> *windows = [NSMutableArray array];
        for (NSWindow *window in NSApp.windows) {
            [windows addObject:@{
                @"class": NSStringFromClass(window.class),
                @"window_number": @(window.windowNumber),
                @"title": window.title ?: @"",
                @"visible": @(window.isVisible),
                @"miniaturizable": @(window.isMiniaturizable),
                @"miniaturized": @(window.isMiniaturized),
                @"style_mask": @(window.styleMask),
                @"occlusion_state": @(window.occlusionState),
                @"matches_rig_layer": @(ck_rig_layer != nil && window.contentView.layer == ck_rig_layer),
            }];
        }
        NSArray<NSDictionary *> *will_rows = ck_window_notifications(ck_will_minimise ?: [NSMutableArray array]);
        NSArray<NSDictionary *> *did_rows = ck_window_notifications(ck_did_minimise ?: [NSMutableArray array]);
        NSDictionary *root = @{
            @"phase": phase_text,
            @"minimize_call_target": ck_minimise_target ?: [NSNull null],
            @"last_poll_target": ck_last_poll_target ?: [NSNull null],
            @"decoy_minimize_call_target": ck_decoy_minimise_target ?: [NSNull null],
            @"windows": windows,
            @"will_miniaturize_count": @(will_rows.count),
            @"will_miniaturize": will_rows,
            @"did_miniaturize_count": @(did_rows.count),
            @"did_miniaturize": did_rows,
            @"notification_observers_installed": @(ck_will_observer != nil && ck_did_observer != nil),
            @"app": @{
                @"active": @(NSApp.isActive),
                @"key_window": ck_window_number(NSApp.keyWindow),
                @"main_window": ck_window_number(NSApp.mainWindow),
                @"dropped_keys": @(atomic_load(&ck_dropped)),
            },
            @"observation": [phase_text isEqualToString:@"observed until T"]
                ? @{
                    @"elapsed_ms": @(observed_ms),
                    @"rig_window_miniaturized": @(observed != 0),
                    @"truncated_by_worker_budget": @(truncated != 0),
                    @"status": observed != 0
                        ? @"rig window miniaturized"
                        : (truncated != 0 ? @"truncated by worker remaining-time budget"
                                          : @"10-second observation budget elapsed"),
                }
                : [NSNull null],
        };
        NSError *error = nil;
        NSData *data = [NSJSONSerialization dataWithJSONObject:root options:0 error:&error];
        if (data == nil) {
            fprintf(stderr, "CK136_MINIMISE_DIAGNOSTIC serialization_error=%s\n",
                    error.localizedDescription.UTF8String ?: "unknown");
        } else {
            fputs("CK136_MINIMISE_DIAGNOSTIC ", stderr);
            fwrite(data.bytes, 1, data.length, stderr);
            fputc('\n', stderr);
        }
        fflush(stderr);
    }
}

/* The layer's retain count, to show a failed start undid its retain (any thread). */
int64_t ck_t_retain_count(void *layer) {
    return (int64_t)CFGetRetainCount(layer);
}

/* Calls a main-thread-only AppKit method from a thread that is not the main thread (the control for the
 * Main Thread Checker). */
void ck_t_violate_main_thread(void) {
    @autoreleasepool {
        [NSApp.windows.firstObject.contentView setNeedsDisplay:YES];
    }
}

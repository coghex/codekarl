/* Shared, test-only activation observer included by each real-window test support archive. */
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <objc/runtime.h>
#include <stdatomic.h>
#include <stdint.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CK_SYNTHETIC_FOCUS_NOTIFICATION @"ck_test_synthetic_focus_notification"

/* A failed pass-through control reports the owner of the unexpected topmost window as well as its number. */
static __attribute__((unused)) pid_t ck_aw_window_owner_pid(NSInteger window_number) {
    CFArrayRef windows = CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID);
    if (windows == NULL) return -1;
    pid_t owner_pid = -1;
    for (CFIndex i = 0; i < CFArrayGetCount(windows); i++) {
        CFDictionaryRef info = (CFDictionaryRef)CFArrayGetValueAtIndex(windows, i);
        int number = 0;
        int pid = -1;
        CFNumberRef number_value = (CFNumberRef)CFDictionaryGetValue(info, kCGWindowNumber);
        CFNumberRef pid_value = (CFNumberRef)CFDictionaryGetValue(info, kCGWindowOwnerPID);
        if (number_value != NULL && pid_value != NULL &&
            CFNumberGetValue(number_value, kCFNumberIntType, &number) && number == window_number &&
            CFNumberGetValue(pid_value, kCFNumberIntType, &pid)) {
            owner_pid = (pid_t)pid;
            break;
        }
    }
    CFRelease(windows);
    return owner_pid;
}

typedef struct {
    uint64_t activate_calls;
    uint64_t activate_ignoring_calls;
    uint64_t make_key_calls;
    uint64_t running_application_activate_calls;
    uint64_t running_application_activate_from_calls;
    uint64_t became_active;
    uint64_t became_key;
    int32_t is_active;
    int32_t has_key_window;
    int32_t is_frontmost;
    int32_t expected_request;
    uint64_t exposed_count;
} CKActivationWatch;

static _Atomic int32_t ck_aw_enabled;
static _Atomic int32_t ck_aw_expected;
static _Atomic uint64_t ck_aw_activate;
static _Atomic uint64_t ck_aw_activate_ignoring;
static _Atomic uint64_t ck_aw_make_key;
static _Atomic uint64_t ck_aw_running_application_activate;
static _Atomic uint64_t ck_aw_running_application_activate_from;
static _Atomic uint64_t ck_aw_exposed;
static _Atomic uint64_t ck_aw_became_active;
static _Atomic uint64_t ck_aw_became_key;
static IMP ck_aw_original_activate;
static IMP ck_aw_original_activate_ignoring;
static IMP ck_aw_original_make_key;
static IMP ck_aw_original_order_front;
static IMP ck_aw_original_order_front_regardless;
static IMP ck_aw_original_order_back;
static IMP ck_aw_original_order_window;
static IMP ck_aw_original_running_application_activate;
static IMP ck_aw_original_running_application_activate_from;
static char *ck_aw_report_path;
static void ck_aw_report(const char *line);

static void ck_aw_record_exposure(NSWindow *window) {
    if (!atomic_load(&ck_aw_enabled) || window.ignoresMouseEvents) return;
    atomic_fetch_add(&ck_aw_exposed, 1);
    char line[96];
    snprintf(line, sizeof line, "exposed=%ld\n", (long)window.windowNumber);
    ck_aw_report(line);
}

static void ck_aw_report(const char *line) {
    if (ck_aw_report_path == NULL) return;
    int fd = open(ck_aw_report_path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd < 0) return;
    size_t length = strlen(line);
    (void)write(fd, line, length);
    (void)close(fd);
}

static void ck_aw_install_interpositions(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        Method activate = class_getInstanceMethod([NSApplication class], @selector(activate));
        ck_aw_original_activate = method_getImplementation(activate);
        method_setImplementation(activate, imp_implementationWithBlock(^(NSApplication *app) {
            if (atomic_load(&ck_aw_enabled)) {
                atomic_fetch_add(&ck_aw_activate, 1);
                ck_aw_report("request=activate\n");
                return;
            }
            ((void (*)(id, SEL))ck_aw_original_activate)(app, @selector(activate));
        }));

        SEL activate_selector = @selector(activateIgnoringOtherApps:);
        Method activate_ignoring = class_getInstanceMethod([NSApplication class], activate_selector);
        ck_aw_original_activate_ignoring = method_getImplementation(activate_ignoring);
        method_setImplementation(activate_ignoring, imp_implementationWithBlock(^(NSApplication *app, BOOL flag) {
            if (atomic_load(&ck_aw_enabled)) {
                atomic_fetch_add(&ck_aw_activate_ignoring, 1);
                ck_aw_report("request=activateIgnoringOtherApps\n");
                return;
            }
            ((void (*)(id, SEL, BOOL))ck_aw_original_activate_ignoring)(app, activate_selector, flag);
        }));

        SEL make_key_selector = @selector(makeKeyAndOrderFront:);
        Method make_key = class_getInstanceMethod([NSWindow class], make_key_selector);
        ck_aw_original_make_key = method_getImplementation(make_key);
        method_setImplementation(make_key, imp_implementationWithBlock(^(NSWindow *window, id sender) {
            if (atomic_load(&ck_aw_enabled)) {
                atomic_fetch_add(&ck_aw_make_key, 1);
                ck_aw_report("request=makeKeyAndOrderFront\n");
                return;
            }
            ((void (*)(id, SEL, id))ck_aw_original_make_key)(window, make_key_selector, sender);
        }));

        Method order_front = class_getInstanceMethod([NSWindow class], @selector(orderFront:));
        ck_aw_original_order_front = method_getImplementation(order_front);
        method_setImplementation(order_front, imp_implementationWithBlock(^(NSWindow *window, id sender) {
            ck_aw_record_exposure(window);
            ((void (*)(id, SEL, id))ck_aw_original_order_front)(window, @selector(orderFront:), sender);
        }));

        Method order_front_regardless = class_getInstanceMethod([NSWindow class], @selector(orderFrontRegardless));
        ck_aw_original_order_front_regardless = method_getImplementation(order_front_regardless);
        method_setImplementation(order_front_regardless, imp_implementationWithBlock(^(NSWindow *window) {
            ck_aw_record_exposure(window);
            ((void (*)(id, SEL))ck_aw_original_order_front_regardless)(window, @selector(orderFrontRegardless));
        }));

        Method order_back = class_getInstanceMethod([NSWindow class], @selector(orderBack:));
        ck_aw_original_order_back = method_getImplementation(order_back);
        method_setImplementation(order_back, imp_implementationWithBlock(^(NSWindow *window, id sender) {
            ck_aw_record_exposure(window);
            ((void (*)(id, SEL, id))ck_aw_original_order_back)(window, @selector(orderBack:), sender);
        }));

        Method order_window = class_getInstanceMethod([NSWindow class], @selector(orderWindow:relativeTo:));
        ck_aw_original_order_window = method_getImplementation(order_window);
        method_setImplementation(order_window, imp_implementationWithBlock(^(NSWindow *window, NSWindowOrderingMode place, NSInteger other) {
            /* NSWindowOut removes a window; it does not expose it to mouse input. */
            if (place != NSWindowOut) ck_aw_record_exposure(window);
            ((void (*)(id, SEL, NSWindowOrderingMode, NSInteger))ck_aw_original_order_window)(window, @selector(orderWindow:relativeTo:), place, other);
        }));

        SEL running_activate_selector = @selector(activateWithOptions:);
        Method running_activate = class_getInstanceMethod([NSRunningApplication class], running_activate_selector);
        ck_aw_original_running_application_activate = method_getImplementation(running_activate);
        method_setImplementation(running_activate, imp_implementationWithBlock(^BOOL(NSRunningApplication *app, NSApplicationActivationOptions options) {
            if (atomic_load(&ck_aw_enabled) && app.processIdentifier == NSProcessInfo.processInfo.processIdentifier) {
                atomic_fetch_add(&ck_aw_running_application_activate, 1);
                ck_aw_report("request=runningApplicationActivate\n");
                return NO;
            }
            return ((BOOL (*)(id, SEL, NSApplicationActivationOptions))ck_aw_original_running_application_activate)(app, running_activate_selector, options);
        }));

        SEL running_from_selector = @selector(activateFromApplication:options:);
        Method running_from = class_getInstanceMethod([NSRunningApplication class], running_from_selector);
        if (running_from != NULL) {
            ck_aw_original_running_application_activate_from = method_getImplementation(running_from);
            method_setImplementation(running_from, imp_implementationWithBlock(^BOOL(NSRunningApplication *app, NSRunningApplication *source, NSApplicationActivationOptions options) {
                if (atomic_load(&ck_aw_enabled) && app.processIdentifier == NSProcessInfo.processInfo.processIdentifier) {
                    atomic_fetch_add(&ck_aw_running_application_activate_from, 1);
                    ck_aw_report("request=runningApplicationActivateFrom\n");
                    return NO;
                }
                return ((BOOL (*)(id, SEL, id, NSApplicationActivationOptions))ck_aw_original_running_application_activate_from)(app, running_from_selector, source, options);
            }));
        }

        NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
        [center addObserverForName:NSApplicationDidBecomeActiveNotification object:nil queue:nil usingBlock:^(NSNotification *note) {
            (void)note;
            if (atomic_load(&ck_aw_enabled)) {
                atomic_fetch_add(&ck_aw_became_active, 1);
                ck_aw_report("outcome=becameActive\n");
            }
        }];
        [center addObserverForName:NSWindowDidBecomeKeyNotification object:nil queue:nil usingBlock:^(NSNotification *note) {
            if ([note.userInfo[CK_SYNTHETIC_FOCUS_NOTIFICATION] boolValue]) return;
            if (atomic_load(&ck_aw_enabled)) {
                atomic_fetch_add(&ck_aw_became_key, 1);
                ck_aw_report("outcome=becameKey\n");
            }
        }];
    });
}

__attribute__((weak)) void ck_activation_watch_set_report_path(const char *path) {
    free(ck_aw_report_path);
    ck_aw_report_path = path != NULL ? strdup(path) : NULL;
}

__attribute__((weak)) void ck_activation_watch_install(int32_t expected_request) {
    @autoreleasepool {
        ck_aw_install_interpositions();
        atomic_store(&ck_aw_activate, 0);
        atomic_store(&ck_aw_activate_ignoring, 0);
        atomic_store(&ck_aw_make_key, 0);
        atomic_store(&ck_aw_running_application_activate, 0);
        atomic_store(&ck_aw_running_application_activate_from, 0);
        atomic_store(&ck_aw_exposed, 0);
        atomic_store(&ck_aw_became_active, 0);
        atomic_store(&ck_aw_became_key, 0);
        atomic_store(&ck_aw_expected, expected_request != 0);
        atomic_store(&ck_aw_enabled, 1);
        char line[64];
        snprintf(line, sizeof line, "pid=%d\n", NSProcessInfo.processInfo.processIdentifier);
        ck_aw_report(line);
        snprintf(line, sizeof line, "expected_request=%d\n", expected_request != 0 ? 1 : 0);
        ck_aw_report(line);
        ck_aw_report("installed=1\n");
    }
}

__attribute__((weak)) void ck_activation_watch_read(CKActivationWatch *out) {
    @autoreleasepool {
        out->activate_calls = atomic_load(&ck_aw_activate);
        out->activate_ignoring_calls = atomic_load(&ck_aw_activate_ignoring);
        out->make_key_calls = atomic_load(&ck_aw_make_key);
        out->running_application_activate_calls = atomic_load(&ck_aw_running_application_activate);
        out->running_application_activate_from_calls = atomic_load(&ck_aw_running_application_activate_from);
        out->exposed_count = atomic_load(&ck_aw_exposed);
        out->became_active = atomic_load(&ck_aw_became_active);
        out->became_key = atomic_load(&ck_aw_became_key);
        out->is_active = NSApp.active ? 1 : 0;
        out->has_key_window = 0;
        for (NSWindow *window in NSApp.windows) {
            if (window.keyWindow) {
                out->has_key_window = 1;
                break;
            }
        }
        out->is_frontmost = NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier == NSProcessInfo.processInfo.processIdentifier ? 1 : 0;
        out->expected_request = atomic_load(&ck_aw_expected);
        char line[160];
        snprintf(line, sizeof line, "end_active=%d end_key=%d end_frontmost=%d\n", out->is_active,
                 out->has_key_window, out->is_frontmost);
        ck_aw_report(line);
        atomic_store(&ck_aw_enabled, 0);
    }
}

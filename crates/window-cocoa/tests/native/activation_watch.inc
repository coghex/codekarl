/* Shared, test-only activation observer included by each real-window test support archive. */
#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <stdatomic.h>
#include <stdint.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CK_SYNTHETIC_FOCUS_NOTIFICATION @"ck_test_synthetic_focus_notification"

typedef struct {
    uint64_t activate_calls;
    uint64_t activate_ignoring_calls;
    uint64_t make_key_calls;
    uint64_t became_active;
    uint64_t became_key;
    int32_t is_active;
    int32_t has_key_window;
    int32_t is_frontmost;
    int32_t expected_request;
} CKActivationWatch;

static _Atomic int32_t ck_aw_enabled;
static _Atomic int32_t ck_aw_expected;
static _Atomic uint64_t ck_aw_activate;
static _Atomic uint64_t ck_aw_activate_ignoring;
static _Atomic uint64_t ck_aw_make_key;
static _Atomic uint64_t ck_aw_became_active;
static _Atomic uint64_t ck_aw_became_key;
static IMP ck_aw_original_activate;
static IMP ck_aw_original_activate_ignoring;
static IMP ck_aw_original_make_key;
static char *ck_aw_report_path;

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
        atomic_store(&ck_aw_became_active, 0);
        atomic_store(&ck_aw_became_key, 0);
        atomic_store(&ck_aw_expected, expected_request != 0);
        atomic_store(&ck_aw_enabled, 1);
        ck_aw_report("installed=1\n");
    }
}

__attribute__((weak)) void ck_activation_watch_read(CKActivationWatch *out) {
    @autoreleasepool {
        out->activate_calls = atomic_load(&ck_aw_activate);
        out->activate_ignoring_calls = atomic_load(&ck_aw_activate_ignoring);
        out->make_key_calls = atomic_load(&ck_aw_make_key);
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

#import <Cocoa/Cocoa.h>
#include <pthread.h>
#include "activation.h"

int ck_glfw_ensure_regular_policy(void) {
    /* Before any application object is touched: nothing of AppKit runs off the main thread. */
    if (pthread_main_np() == 0) {
        return CK_GLFW_OFF_MAIN;
    }
    @autoreleasepool {
        NSApplication *application = [NSApplication sharedApplication];
        /* Set once: a second backend in the process finds the policy already in place (and AppKit then
         * refuses to set it again). */
        if (application.activationPolicy != NSApplicationActivationPolicyRegular &&
            ![application setActivationPolicy:NSApplicationActivationPolicyRegular]) {
            return CK_GLFW_POLICY_REFUSED;
        }
        return CK_GLFW_OK;
    }
}

int ck_glfw_activate_window(void *native_window) {
    if (pthread_main_np() == 0) {
        return CK_GLFW_OFF_MAIN;
    }
    @autoreleasepool {
        NSWindow *window = (__bridge NSWindow *)native_window;
        if (native_window == NULL || ![window isKindOfClass:[NSWindow class]]) {
            return CK_GLFW_NO_WINDOW;
        }
        if (@available(macOS 14.0, *)) {
            [NSApp activate];
        } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            [NSApp activateIgnoringOtherApps:YES];
#pragma clang diagnostic pop
        }
        [window makeKeyAndOrderFront:nil];
        return CK_GLFW_OK;
    }
}

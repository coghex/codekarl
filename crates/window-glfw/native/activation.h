#ifndef CK_WINDOW_GLFW_ACTIVATION_H
#define CK_WINDOW_GLFW_ACTIVATION_H

/* The two AppKit calls the GLFW backend makes itself (macOS only). Both take the process's main thread and
 * return a status; neither waits. This file is part of the library, unlike tests/native. */

/* Status codes shared with src/activation.rs. */
#define CK_GLFW_OK 0
#define CK_GLFW_OFF_MAIN 1
#define CK_GLFW_POLICY_REFUSED 2
#define CK_GLFW_NO_WINDOW 3

/* Gives the process the regular activation policy unless it already has it. GLFW sets that policy only
 * together with its menu bar, which the backend leaves out (GLFW_COCOA_MENUBAR false), so without this the
 * process has no Dock icon, is not in Cmd-Tab and cannot become the active application.
 * CK_GLFW_POLICY_REFUSED when AppKit refuses the policy. */
int ck_glfw_ensure_regular_policy(void);

/* Asks the system to make the application active (a request it may decline) and makes `native_window` (an
 * NSWindow, as glfwGetCocoaWindow returns it) key and orders it front. Does not wait for the answer.
 * CK_GLFW_NO_WINDOW when `native_window` is not an NSWindow. */
int ck_glfw_activate_window(void *native_window);

#endif

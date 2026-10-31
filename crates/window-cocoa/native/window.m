#import "internal.h"
#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>

static void ck_error(char *buffer, size_t capacity, NSString *message) {
    if (buffer != NULL && capacity != 0) {
        snprintf(buffer, capacity, "%s", message.UTF8String ?: "Unknown window error");
    }
}

static BOOL ck_is_wake(NSEvent *event) {
    return event.type == NSEventTypeApplicationDefined && event.subtype == CK_WAKE_SUBTYPE &&
           event.data1 == CK_WAKE_MAGIC;
}

/* ---------------------------------------------------------------------------------------------
 * The key table
 * ------------------------------------------------------------------------------------------- */

/* One table maps the hardware key code of an event (a macOS virtual key code, the ANSI positions for letters, digits
 * and punctuation; SDK Carbon/HIToolbox/Events.h kVK_*) to a ck-input key: its number is its position in ck-input's
 * `Key::ALL`, and the name is the W3C UI Events `code` the tests check against `Key::name`. A code outside the table
 * is dropped. Modifiers: 56/60 Shift, 59/62 Control, 58/61 Option (Alt), 55/54 Command (Meta), left/right.
 * Keypad keys, Caps Lock and Fn are not in this milestone's key set. */
typedef struct {
    uint16_t code;
    uint8_t key;
    const char *name;
} CKKeyEntry;

static const CKKeyEntry ck_key_table[CK_WIN_KEY_COUNT] = {
    {0x00,  0, "KeyA"},
    {0x0B,  1, "KeyB"},
    {0x08,  2, "KeyC"},
    {0x02,  3, "KeyD"},
    {0x0E,  4, "KeyE"},
    {0x03,  5, "KeyF"},
    {0x05,  6, "KeyG"},
    {0x04,  7, "KeyH"},
    {0x22,  8, "KeyI"},
    {0x26,  9, "KeyJ"},
    {0x28, 10, "KeyK"},
    {0x25, 11, "KeyL"},
    {0x2E, 12, "KeyM"},
    {0x2D, 13, "KeyN"},
    {0x1F, 14, "KeyO"},
    {0x23, 15, "KeyP"},
    {0x0C, 16, "KeyQ"},
    {0x0F, 17, "KeyR"},
    {0x01, 18, "KeyS"},
    {0x11, 19, "KeyT"},
    {0x20, 20, "KeyU"},
    {0x09, 21, "KeyV"},
    {0x0D, 22, "KeyW"},
    {0x07, 23, "KeyX"},
    {0x10, 24, "KeyY"},
    {0x06, 25, "KeyZ"},
    {0x1D, 26, "Digit0"},
    {0x12, 27, "Digit1"},
    {0x13, 28, "Digit2"},
    {0x14, 29, "Digit3"},
    {0x15, 30, "Digit4"},
    {0x17, 31, "Digit5"},
    {0x16, 32, "Digit6"},
    {0x1A, 33, "Digit7"},
    {0x1C, 34, "Digit8"},
    {0x19, 35, "Digit9"},
    {0x7A, 36, "F1"},
    {0x78, 37, "F2"},
    {0x63, 38, "F3"},
    {0x76, 39, "F4"},
    {0x60, 40, "F5"},
    {0x61, 41, "F6"},
    {0x62, 42, "F7"},
    {0x64, 43, "F8"},
    {0x65, 44, "F9"},
    {0x6D, 45, "F10"},
    {0x67, 46, "F11"},
    {0x6F, 47, "F12"},
    {0x7E, 48, "ArrowUp"},
    {0x7D, 49, "ArrowDown"},
    {0x7B, 50, "ArrowLeft"},
    {0x7C, 51, "ArrowRight"},
    {0x35, 52, "Escape"},
    {0x31, 53, "Space"},
    {0x24, 54, "Enter"},
    {0x30, 55, "Tab"},
    {0x33, 56, "Backspace"},
    {0x75, 57, "Delete"},
    {0x73, 58, "Home"},
    {0x77, 59, "End"},
    {0x74, 60, "PageUp"},
    {0x79, 61, "PageDown"},
    {0x1B, 62, "Minus"},
    {0x18, 63, "Equal"},
    {0x21, 64, "BracketLeft"},
    {0x1E, 65, "BracketRight"},
    {0x2A, 66, "Backslash"},
    {0x29, 67, "Semicolon"},
    {0x27, 68, "Quote"},
    {0x2B, 69, "Comma"},
    {0x2F, 70, "Period"},
    {0x2C, 71, "Slash"},
    {0x32, 72, "Backquote"},
    {0x38, 73, "ShiftLeft"},
    {0x3C, 74, "ShiftRight"},
    {0x3B, 75, "ControlLeft"},
    {0x3E, 76, "ControlRight"},
    {0x3A, 77, "AltLeft"},
    {0x3D, 78, "AltRight"},
    {0x37, 79, "MetaLeft"},
    {0x36, 80, "MetaRight"},
};

NSUInteger ck_win_key_table_count(void) {
    return CK_WIN_KEY_COUNT;
}

BOOL ck_win_key_table_entry(NSUInteger position, uint16_t *code, uint8_t *key, const char **name) {
    if (position >= CK_WIN_KEY_COUNT) {
        return NO;
    }
    *code = ck_key_table[position].code;
    *key = ck_key_table[position].key;
    *name = ck_key_table[position].name;
    return YES;
}

/* The key number of a virtual key code, or -1. Built once from the table. */
static int ck_key_for_code(uint16_t code) {
    static int8_t index_by_code[128];
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        memset(index_by_code, -1, sizeof index_by_code);
        for (size_t i = 0; i < CK_WIN_KEY_COUNT; i++) {
            index_by_code[ck_key_table[i].code] = (int8_t)ck_key_table[i].key;
        }
    });
    return code < 128 ? index_by_code[code] : -1;
}

/* The device-dependent side bits in the low bits of NSEvent.modifierFlags, named after IOKit's
 * hidsystem/IOLLEvent.h (NX_DEVICE*KEYMASK):
 *   NX_DEVICELCTLKEYMASK 0x00000001   NX_DEVICERCTLKEYMASK 0x00002000
 *   NX_DEVICELSHIFTKEYMASK 0x00000002 NX_DEVICERSHIFTKEYMASK 0x00000004
 *   NX_DEVICELCMDKEYMASK 0x00000008   NX_DEVICERCMDKEYMASK 0x00000010
 *   NX_DEVICELALTKEYMASK 0x00000020   NX_DEVICERALTKEYMASK 0x00000040
 * and the family bits are AppKit/NSEvent.h's NSEventModifierFlagShift/Control/Option/Command. */
#define CK_DEVICE_LCTL 0x00000001
#define CK_DEVICE_LSHIFT 0x00000002
#define CK_DEVICE_RSHIFT 0x00000004
#define CK_DEVICE_LCMD 0x00000008
#define CK_DEVICE_RCMD 0x00000010
#define CK_DEVICE_LALT 0x00000020
#define CK_DEVICE_RALT 0x00000040
#define CK_DEVICE_RCTL 0x00002000

/* The families, in the order of the tables below. */
enum { CK_SHIFT, CK_CONTROL, CK_ALT, CK_META, CK_FAMILIES };
static const uint16_t ck_left_code[CK_FAMILIES] = {56, 59, 58, 55};
static const uint16_t ck_right_code[CK_FAMILIES] = {60, 62, 61, 54};
static const NSUInteger ck_left_bit[CK_FAMILIES] = {CK_DEVICE_LSHIFT, CK_DEVICE_LCTL, CK_DEVICE_LALT,
                                                    CK_DEVICE_LCMD};
static const NSUInteger ck_right_bit[CK_FAMILIES] = {CK_DEVICE_RSHIFT, CK_DEVICE_RCTL, CK_DEVICE_RALT,
                                                     CK_DEVICE_RCMD};
static const NSUInteger ck_family_bit[CK_FAMILIES] = {
    NSEventModifierFlagShift, NSEventModifierFlagControl, NSEventModifierFlagOption,
    NSEventModifierFlagCommand};

static BOOL ck_get(const uint64_t *words, uint8_t key) {
    return (words[key / 64] & (UINT64_C(1) << (key % 64))) != 0;
}

static void ck_set(uint64_t *words, uint8_t key, BOOL value) {
    if (value) {
        words[key / 64] |= UINT64_C(1) << (key % 64);
    } else {
        words[key / 64] &= ~(UINT64_C(1) << (key % 64));
    }
}

/* ---------------------------------------------------------------------------------------------
 * The wake target
 * ------------------------------------------------------------------------------------------- */

@implementation CKWakeBox {
    _Atomic bool _queued;
}

- (instancetype)initWithApplication:(NSApplication *)application {
    self = [super init];
    if (self != nil) {
        _application = application;
        atomic_init(&_queued, false);
    }
    return self;
}

/* Any thread. The queued flag is set before the event is posted and cleared when the pump takes a wake
 * event off the queue (and at the start of each pump), so a wake made after the clear queues a new
 * event: no wake is lost, and wake events do not pile up while one is waiting. */
- (void)post {
    @autoreleasepool {
        if (atomic_exchange(&_queued, true)) {
            return;
        }
        NSEvent *event = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
                                            location:NSZeroPoint
                                       modifierFlags:0
                                           timestamp:0
                                        windowNumber:0
                                             context:nil
                                             subtype:CK_WAKE_SUBTYPE
                                               data1:CK_WAKE_MAGIC
                                               data2:0];
        if (event == nil) {
            atomic_store(&_queued, false);
            return;
        }
        [self.application postEvent:event atStart:NO];
    }
}

- (void)clearQueued {
    atomic_store(&_queued, false);
}

@end

/* ---------------------------------------------------------------------------------------------
 * The application, window, view and delegate
 * ------------------------------------------------------------------------------------------- */

@implementation CKWinView

- (BOOL)acceptsFirstResponder {
    return YES;
}

- (BOOL)wantsUpdateLayer {
    return YES;
}

/* The view's backing layer is the Metal layer the presenter draws to. */
- (CALayer *)makeBackingLayer {
    return [CAMetalLayer layer];
}

/* Every key event is recorded (a code outside the key table is dropped) and consumed here, so none reaches the
 * next responder and none beeps. Cmd-Q and every other key equivalent stay what they were: nothing (no menu). */
- (void)keyDown:(NSEvent *)event {
    [self.state recordKeyCode:event.keyCode down:YES repeat:event.isARepeat];
}

- (void)keyUp:(NSEvent *)event {
    [self.state recordKeyCode:event.keyCode down:NO repeat:NO];
}

- (void)flagsChanged:(NSEvent *)event {
    [self.state recordFlagsChangedCode:event.keyCode flags:event.modifierFlags];
}

@end

@implementation CKWinDelegate

/* The window never closes by itself: the request is recorded and refused. */
- (BOOL)windowShouldClose:(NSWindow *)sender {
    (void)sender;
    [self.state recordClose:CK_WIN_CLOSE_BUTTON];
    return NO;
}

- (void)windowDidResignKey:(NSNotification *)notification {
    (void)notification;
    [self.state recordFocusLost];
}

- (void)windowDidBecomeKey:(NSNotification *)notification {
    (void)notification;
    [self.state recordFocusGained];
}

- (void)windowDidChangeBackingProperties:(NSNotification *)notification {
    (void)notification;
    [self.state applyScaleAndRecordPixelSize];
}

- (void)windowDidChangeOcclusionState:(NSNotification *)notification {
    (void)notification;
    NSWindow *window = self.state.window;
    if (window != nil && (window.occlusionState & NSWindowOcclusionStateVisible) != 0) {
        [self.state recordNeedsDisplay];
    }
}

- (void)windowDidDeminiaturize:(NSNotification *)notification {
    (void)notification;
    [self.state recordRestored];
}

- (void)windowDidMiniaturize:(NSNotification *)notification {
    (void)notification;
    [self.state recordMinimized];
}

@end

@implementation CKWinState

- (BOOL)isPending {
    return self.closeSource != CK_WIN_CLOSE_NONE || self.pixelChanged || self.needsDisplay ||
           self.minimizedPending || self.restoredPending || _ringLength != 0 || _overflow || _focusLostPending ||
           _focusGainedPending;
}

/* Every record goes through here: a move from an empty to a non-empty pending state posts one wake, so a
 * pump already waiting in nextEventMatchingMask returns with the recorded work. */
- (void)afterRecord:(BOOL)wasEmpty {
    if (wasEmpty) {
        [self.wake post];
    }
}

- (void)recordClose:(int32_t)source {
    BOOL wasEmpty = ![self isPending];
    if (self.closeSource == CK_WIN_CLOSE_NONE) {
        self.closeSource = source;
    } else if (self.closeAbsorbed < UINT32_MAX) {
        self.closeAbsorbed += 1;
    }
    [self afterRecord:wasEmpty];
}

- (void)recordNeedsDisplay {
    BOOL wasEmpty = ![self isPending];
    self.needsDisplay = YES;
    [self afterRecord:wasEmpty];
}

- (void)recordMinimized {
    BOOL wasEmpty = ![self isPending];
    self.minimizedPending = YES;
    [self afterRecord:wasEmpty];
}

- (void)recordRestored {
    BOOL wasEmpty = ![self isPending];
    self.restoredPending = YES;
    self.needsDisplay = YES;
    [self afterRecord:wasEmpty];
}

/* ---- keys: constant work, no allocation, nothing logged ---- */

/* Moves the oldest ring entry into the loss latches. Only a discarded transition sets a latch, and a repeat is no
 * press (its first down is delivered or latched itself). */
- (void)discardOldest {
    CKWinTransition lost = _ring[_ringHead];
    _ringHead = (_ringHead + 1) % CK_WIN_RING_CAPACITY;
    _ringLength -= 1;
    if (_focusGainedPending && _gainAt != 0) {
        /* The discarded record was recorded before the pending gain. */
        _gainAt -= 1;
        _gainAfterLost = YES;
    }
    if (!lost.down) {
        ck_set(_lostReleased, lost.key, YES);
    } else if (!lost.repeat) {
        ck_set(_lostPressed, lost.key, YES);
    }
}

/* One transition: the held bitset is updated at once (it is exact whatever happens to the ring); a full ring
 * discards its oldest entry into the latches and flags the overflow. The caller does the wake. */
- (void)appendKey:(uint8_t)key down:(BOOL)down repeat:(BOOL)repeat {
    ck_set(_held, key, down);
    if (_ringLength == CK_WIN_RING_CAPACITY) {
        [self discardOldest];
        _overflow = YES;
        if (_overflowTotal < UINT64_MAX) {
            _overflowTotal += 1;
        }
    }
    uint32_t tail = (_ringHead + _ringLength) % CK_WIN_RING_CAPACITY;
    _ring[tail] = (CKWinTransition){.key = key, .down = down ? 1 : 0, .repeat = repeat ? 1 : 0};
    _ringLength += 1;
}

- (void)recordKeyCode:(uint16_t)code down:(BOOL)down repeat:(BOOL)repeat {
    int key = ck_key_for_code(code);
    if (key < 0) {
        return;
    }
    BOOL wasEmpty = ![self isPending];
    [self appendKey:(uint8_t)key down:down repeat:repeat];
    [self afterRecord:wasEmpty];
}

/* A modifier change names its key by code. The side is decided from the device-dependent side bits for that side
 * (down when its bit is set, up when clear). With no side bit of the family in the event (some virtual or remote
 * input), the code toggles the side's state. Either way, whenever the family's own bit is clear both sides of the
 * family are released, so a missed release can never leave a modifier held past the family's release. */
- (void)recordFlagsChangedCode:(uint16_t)code flags:(NSUInteger)flags {
    int family = -1;
    BOOL right = NO;
    for (int f = 0; f < CK_FAMILIES; f++) {
        if (code == ck_left_code[f]) {
            family = f;
        } else if (code == ck_right_code[f]) {
            family = f;
            right = YES;
        }
    }
    if (family < 0) {
        return;
    }
    int self_key = ck_key_for_code(code);
    int other_key = ck_key_for_code(right ? ck_left_code[family] : ck_right_code[family]);
    if (self_key < 0 || other_key < 0) {
        return;
    }
    BOOL side_bits = (flags & (ck_left_bit[family] | ck_right_bit[family])) != 0;
    BOOL self_down;
    if (side_bits) {
        self_down = (flags & (right ? ck_right_bit[family] : ck_left_bit[family])) != 0;
    } else {
        self_down = !ck_get(_held, (uint8_t)self_key);
    }
    BOOL family_down = (flags & ck_family_bit[family]) != 0;
    if (!family_down) {
        self_down = NO;
    }
    BOOL wasEmpty = ![self isPending];
    if (self_down != ck_get(_held, (uint8_t)self_key)) {
        [self appendKey:(uint8_t)self_key down:self_down repeat:NO];
    }
    if (!family_down && ck_get(_held, (uint8_t)other_key)) {
        [self appendKey:(uint8_t)other_key down:NO repeat:NO];
    }
    [self afterRecord:wasEmpty];
}

/* Forgets every key: the held bitset, the ring, the latches and the overflow flag. */
- (void)resetKeys {
    memset(_held, 0, sizeof _held);
    memset(_lostPressed, 0, sizeof _lostPressed);
    memset(_lostReleased, 0, sizeof _lostReleased);
    _ringHead = 0;
    _ringLength = 0;
    _overflow = NO;
    _focusGainedPending = NO;
    _gainAt = 0;
    _gainAfterLost = NO;
}

/* Focus loss clears the native key state too, and is a pending flag, not a ring entry: the next pump delivers it
 * before anything recorded after it, and nothing recorded before it is resurrected. */
- (void)recordFocusLost {
    BOOL wasEmpty = ![self isPending];
    [self resetKeys];
    _focusLostPending = YES;
    [self afterRecord:wasEmpty];
}

/* A gain is a pending flag with a position, not a ring entry (so it cannot be discarded): `_gainAt` ring records
 * were recorded before it and are still in the ring, and `_gainAfterLost` says that detail recorded before it was
 * already discarded. Bounded: one flag, one counter, one flag. A second gain without a loss in between keeps the
 * first one's place. */
- (void)recordFocusGained {
    BOOL wasEmpty = ![self isPending];
    if (!_focusGainedPending) {
        _focusGainedPending = YES;
        _gainAt = _ringLength;
        /* In an overflow not yet handed over, detail was already discarded before this gain; the ring then still holds
         * records ahead of it (`_gainAt` is not 0), which is what puts the resync first. */
        _gainAfterLost = NO;
    }
    [self afterRecord:wasEmpty];
}

/* What a pump hands over of the key state, at most `cap` entries, in recorded order: a pending focus loss, then the
 * ring's records in order with a pending focus gain at the place it was recorded (`_gainAt` records in), or - after
 * an overflow - no ordered detail at all, only one resync. The ring's surviving records follow a lost prefix, so
 * they are discarded into the latches as well; the resync carries every key's final held state and the two
 * latches, which are then cleared, and it stands where its first lost record stood: after a gain recorded before
 * all lost detail, before a gain recorded after some of it. Every transition is delivered once as a key entry or
 * discarded once into the latches, never both. */
- (void)takeKeysInto:(CKWinEvents *)out cap:(size_t)cap {
    uint32_t count = 0;
    if (_focusLostPending && count < cap) {
        out->out[count++] = (CKWinOut){.kind = CK_WIN_OUT_FOCUS_LOST};
        _focusLostPending = NO;
    }
    if (!_focusLostPending) {
        if (_overflow) {
            if (_focusGainedPending && _gainAt == 0 && !_gainAfterLost && count < cap) {
                out->out[count++] = (CKWinOut){.kind = CK_WIN_OUT_FOCUS_GAINED};
                _focusGainedPending = NO;
            }
            if (count < cap && (!_focusGainedPending || _gainAt != 0 || _gainAfterLost)) {
                while (_ringLength != 0) {
                    [self discardOldest];
                }
                memcpy(out->resync.held, _held, sizeof _held);
                memcpy(out->resync.pressed, _lostPressed, sizeof _lostPressed);
                memcpy(out->resync.released, _lostReleased, sizeof _lostReleased);
                out->out[count++] = (CKWinOut){.kind = CK_WIN_OUT_RESYNC};
                memset(_lostPressed, 0, sizeof _lostPressed);
                memset(_lostReleased, 0, sizeof _lostReleased);
                _overflow = NO;
                /* What is left of a gain recorded after lost detail now comes right after the resync. */
                _gainAt = 0;
                _gainAfterLost = NO;
            }
        }
        while (!_overflow && count < cap) {
            if (_focusGainedPending && _gainAt == 0) {
                out->out[count++] = (CKWinOut){.kind = CK_WIN_OUT_FOCUS_GAINED};
                _focusGainedPending = NO;
                continue;
            }
            if (_ringLength == 0) {
                break;
            }
            CKWinTransition record = _ring[_ringHead];
            _ringHead = (_ringHead + 1) % CK_WIN_RING_CAPACITY;
            _ringLength -= 1;
            if (_focusGainedPending) {
                _gainAt -= 1;
            }
            out->out[count++] = (CKWinOut){.kind = CK_WIN_OUT_KEY,
                                           .key = record.key,
                                           .down = record.down,
                                           .repeat = record.repeat};
        }
    }
    out->out_count = count;
    out->overflow_total = _overflowTotal;
}

- (uint64_t)overflowTotal {
    return _overflowTotal;
}

- (void)nativeKeys:(CKWinNativeKeys *)out {
    memcpy(out->held, _held, sizeof _held);
    memcpy(out->lost_pressed, _lostPressed, sizeof _lostPressed);
    memcpy(out->lost_released, _lostReleased, sizeof _lostReleased);
    out->ring_length = _ringLength;
    out->overflow = _overflow ? 1 : 0;
    out->focus_lost_pending = _focusLostPending ? 1 : 0;
    out->focus_gained_pending = _focusGainedPending ? 1 : 0;
    out->overflow_total = _overflowTotal;
}

- (double)scale {
    return self.window != nil ? self.window.backingScaleFactor : 1.0;
}

- (void)applyScaleAndRecordPixelSize {
    if (self.window == nil || self.view == nil) {
        return;
    }
    double scale = [self scale];
    self.view.layer.contentsScale = scale;
    NSSize points = self.view.bounds.size;
    uint32_t width = (uint32_t)llround(points.width * scale);
    uint32_t height = (uint32_t)llround(points.height * scale);
    if (width == self.knownWidth && height == self.knownHeight) {
        return;
    }
    BOOL wasEmpty = ![self isPending];
    self.knownWidth = width;
    self.knownHeight = height;
    self.pixelChanged = YES;
    self.pixelWidth = width;
    self.pixelHeight = height;
    [self afterRecord:wasEmpty];
}

- (void)clearPending {
    self.closeSource = CK_WIN_CLOSE_NONE;
    self.closeAbsorbed = 0;
    self.pixelChanged = NO;
    self.pixelWidth = 0;
    self.pixelHeight = 0;
    self.needsDisplay = NO;
    self.minimizedPending = NO;
    self.restoredPending = NO;
}

/* Orders the window out, closes it and releases the window, view, layer and delegate. */
- (void)destroyWindow {
    NSWindow *window = self.window;
    if (window == nil) {
        return;
    }
    window.delegate = nil;
    [window orderOut:nil];
    [window close];
    window.contentView = nil;
    self.window = nil;
    self.view = nil;
    self.delegate = nil;
    self.knownWidth = 0;
    self.knownHeight = 0;
    [self clearPending];
    [self resetKeys];
    _focusLostPending = NO;
    _focusGainedPending = NO;
}

@end

/* ---------------------------------------------------------------------------------------------
 * The C interface
 * ------------------------------------------------------------------------------------------- */

size_t ck_win_events_size(void) {
    return sizeof(CKWinEvents);
}

CKWinHandle *ck_win_create(char *error, size_t capacity) {
    /* Before any application object exists: nothing of AppKit is touched off the main thread. */
    if (pthread_main_np() == 0) {
        ck_error(error, capacity, @"the window backend must be created on the process's main thread");
        return NULL;
    }
    @autoreleasepool {
        NSApplication *application = [NSApplication sharedApplication];
        /* Set once: a second backend in the process finds the policy already in place (and AppKit then
         * refuses to set it again). */
        if (application.activationPolicy != NSApplicationActivationPolicyRegular &&
            ![application setActivationPolicy:NSApplicationActivationPolicyRegular]) {
            ck_error(error, capacity, @"the regular activation policy could not be set");
            return NULL;
        }
        /* What [NSApp run] would do first, once for the process, however many backends are created. */
        static dispatch_once_t launched;
        dispatch_once(&launched, ^{
            [application finishLaunching];
        });
        CKWinState *state = [CKWinState new];
        state.application = application;
        state.wake = [[CKWakeBox alloc] initWithApplication:application];
        return (__bridge_retained CKWinHandle *)state;
    }
}

int ck_win_create_window(CKWinHandle *handle, const char *title, size_t title_length,
                         uint32_t width, uint32_t height, void **layer,
                         uint32_t *pixel_width, uint32_t *pixel_height, char *error,
                         size_t capacity) {
    @autoreleasepool {
        CKWinState *state = (__bridge CKWinState *)handle;
        if (state.terminated) {
            ck_error(error, capacity, @"the backend was terminated");
            return 1;
        }
        if (state.window != nil) {
            ck_error(error, capacity, @"a window already exists");
            return 1;
        }
        NSString *text = [[NSString alloc] initWithBytes:title
                                                  length:title_length
                                                encoding:NSUTF8StringEncoding];
        if (text == nil) {
            ck_error(error, capacity, @"the window title is not valid UTF-8");
            return 1;
        }
        NSRect content = NSMakeRect(0, 0, width, height);
        /* Title, close and minimise buttons; not resizable. */
        NSWindowStyleMask mask = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                 NSWindowStyleMaskMiniaturizable;
        NSWindow *window = [[NSWindow alloc] initWithContentRect:content
                                                       styleMask:mask
                                                         backing:NSBackingStoreBuffered
                                                           defer:NO];
        if (window == nil) {
            ck_error(error, capacity, @"the window could not be created");
            return 1;
        }
        window.releasedWhenClosed = NO;
        /* Without this, the show animation keeps the window alive (and listed in [NSApp windows]) after it
         * is closed and released. */
        window.animationBehavior = NSWindowAnimationBehaviorNone;
        window.title = text;
        CKWinView *view = [[CKWinView alloc] initWithFrame:content];
        view.state = state;
        view.wantsLayer = YES;
        if (![view.layer isKindOfClass:[CAMetalLayer class]]) {
            ck_error(error, capacity, @"the view's backing layer is not a CAMetalLayer");
            return 1;
        }
        CKWinDelegate *delegate = [CKWinDelegate new];
        delegate.state = state;
        window.contentView = view;
        window.delegate = delegate;
        double scale = window.backingScaleFactor;
        view.layer.contentsScale = scale;
        uint32_t pixels_wide = (uint32_t)llround(width * scale);
        uint32_t pixels_high = (uint32_t)llround(height * scale);
        state.window = window;
        state.view = view;
        state.delegate = delegate;
        state.knownWidth = pixels_wide;
        state.knownHeight = pixels_high;
        [window center];
        [window makeFirstResponder:view];
        [window makeKeyAndOrderFront:nil];
        if (@available(macOS 14.0, *)) {
            [state.application activate];
        } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            [state.application activateIgnoringOtherApps:YES];
#pragma clang diagnostic pop
        }
        *layer = (__bridge void *)view.layer;
        *pixel_width = pixels_wide;
        *pixel_height = pixels_high;
        return 0;
    }
}

static void ck_win_pump_impl(CKWinHandle *handle, int wait, size_t cap, BOOL timed, double timeout_seconds,
                             CKWinEvents *out) {
    @autoreleasepool {
        CKWinState *state = (__bridge CKWinState *)handle;
        memset(out, 0, sizeof *out);
        if (state.terminated) {
            return;
        }
        NSApplication *application = state.application;
        /* A stale flag (an event another loop took) must not swallow later wakes. */
        [state.wake clearQueued];
        /* Pending state first: never sleep with work already recorded. */
        BOOL block = wait != 0 && ![state isPending];
        /* The key and focus state recorded before this pump is handed over now, before dispatch; what dispatch
         * records belongs to the next pump (and keeps the pump from sleeping). */
        [state takeKeysInto:out cap:cap];
        uint64_t taken = 0;
        while (taken < cap) {
            BOOL first = taken == 0 && block;
            NSDate *limit = first ? (timed ? [NSDate dateWithTimeIntervalSinceNow:fmax(0.0, timeout_seconds)]
                                            : [NSDate distantFuture])
                                  : [NSDate distantPast];
            NSEvent *event = [application nextEventMatchingMask:NSEventMaskAny
                                                      untilDate:limit
                                                         inMode:NSDefaultRunLoopMode
                                                        dequeue:YES];
            if (event == nil) {
                break;
            }
            taken += 1;
            if (ck_is_wake(event)) {
                [state.wake clearQueued];
                continue;
            }
            [application sendEvent:event];
        }
        /* What a pass of [NSApp run] does after dispatch; its callbacks join this hand-over. */
        [application updateWindows];
        out->close_source = state.closeSource;
        out->close_absorbed = state.closeAbsorbed;
        out->pixel_changed = state.pixelChanged ? 1 : 0;
        out->pixel_width = state.pixelWidth;
        out->pixel_height = state.pixelHeight;
        out->needs_display = state.needsDisplay ? 1 : 0;
        out->minimized = state.minimizedPending ? 1 : 0;
        out->restored = state.restoredPending ? 1 : 0;
        out->native_events = taken;
        [state clearPending];
        out->overflow_total = [state overflowTotal];
    }
}

void ck_win_pump(CKWinHandle *handle, int wait, size_t cap, CKWinEvents *out) {
    ck_win_pump_impl(handle, wait, cap, NO, 0.0, out);
}

void ck_win_pump_until(CKWinHandle *handle, size_t cap, double timeout_seconds, CKWinEvents *out) {
    ck_win_pump_impl(handle, 1, cap, YES, timeout_seconds, out);
}

void ck_win_destroy_window(CKWinHandle *handle) {
    @autoreleasepool {
        [(__bridge CKWinState *)handle destroyWindow];
    }
}

void ck_win_terminate(CKWinHandle *handle) {
    @autoreleasepool {
        CKWinState *state = (__bridge CKWinState *)handle;
        [state destroyWindow];
        /* The backend's reference; wake references already handed out keep their own. */
        state.wake = nil;
        state.terminated = YES;
    }
}

void ck_win_free(CKWinHandle *handle) {
    if (handle == NULL) {
        return;
    }
    ck_win_terminate(handle);
    @autoreleasepool {
        __unused CKWinState *state = (__bridge_transfer CKWinState *)handle;
    }
}

CKWinWakeHandle *ck_win_wake_target(CKWinHandle *handle) {
    @autoreleasepool {
        CKWinState *state = (__bridge CKWinState *)handle;
        CKWakeBox *box = state.wake;
        if (box == nil) {
            box = [[CKWakeBox alloc] initWithApplication:state.application];
        }
        return (__bridge_retained CKWinWakeHandle *)box;
    }
}

void ck_win_wake(const CKWinWakeHandle *wake) {
    @autoreleasepool {
        [(__bridge CKWakeBox *)wake post];
    }
}

void ck_win_wake_release(CKWinWakeHandle *wake) {
    if (wake != NULL) {
        @autoreleasepool {
            __unused CKWakeBox *box = (__bridge_transfer CKWakeBox *)wake;
        }
    }
}

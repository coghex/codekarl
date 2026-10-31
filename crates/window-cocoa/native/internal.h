#ifndef CK_WINDOW_COCOA_INTERNAL_H
#define CK_WINDOW_COCOA_INTERNAL_H
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#include "window.h"

/* This backend's own wake events: application-defined, with this subtype and magic in data1. */
#define CK_WAKE_SUBTYPE 0x4B57
#define CK_WAKE_MAGIC 0x434B57414B450001LL

@class CKWinState;

/* The wake target: a strong reference to the application, and a flag that coalesces wake events while
 * one is queued. Its only operations are an atomic exchange and NSApplication postEvent:atStart:, which
 * Apple documents as callable from any thread. */
@interface CKWakeBox : NSObject
@property(nonatomic, strong, readonly) NSApplication *application;
- (instancetype)initWithApplication:(NSApplication *)application;
- (void)post;
- (void)clearQueued;
@end

@interface CKWinView : NSView
@property(nonatomic, weak) CKWinState *state;
@end

/* One recorded key transition (the ring's entry). */
typedef struct {
    uint8_t key;
    uint8_t down;
    uint8_t repeat;
} CKWinTransition;

/* The native key state, for the tests: what the responder has recorded and not yet handed over. */
typedef struct {
    uint64_t held[2];
    uint64_t lost_pressed[2];
    uint64_t lost_released[2];
    uint32_t ring_length;
    int32_t overflow;
    int32_t focus_lost_pending;
    int32_t focus_gained_pending;
    uint64_t overflow_total;
} CKWinNativeKeys;

/* The table of keys: macOS virtual key code, the key's number (its position in ck-input's `Key::ALL`) and its W3C
 * `code` name. For the lookup and for the tests, which check it against ck-input. */
NSUInteger ck_win_key_table_count(void);
BOOL ck_win_key_table_entry(NSUInteger position, uint16_t *code, uint8_t *key, const char **name);

@interface CKWinDelegate : NSObject <NSWindowDelegate>
@property(nonatomic, weak) CKWinState *state;
@end

/* The backend: callbacks only update the fixed-size pending state below. The key state is recorded in constant work:
 * an authoritative held-key bitset (exact), a ring of ordered transitions (the detail, bounded), and per-key loss
 * latches that a transition sets only when it is discarded from the ring. */
@interface CKWinState : NSObject {
    uint64_t _held[2];
    uint64_t _lostPressed[2];
    uint64_t _lostReleased[2];
    CKWinTransition _ring[CK_WIN_RING_CAPACITY];
    uint32_t _ringHead;
    uint32_t _ringLength;
    BOOL _overflow;
    uint64_t _overflowTotal;
    BOOL _focusLostPending;
    BOOL _focusGainedPending;
    uint32_t _gainAt;
    BOOL _gainAfterLost;
}
@property(nonatomic, strong) NSApplication *application;
@property(nonatomic, strong) CKWakeBox *wake;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, strong) CKWinView *view;
@property(nonatomic, strong) CKWinDelegate *delegate;
@property(nonatomic) BOOL terminated;
@property(nonatomic) int32_t closeSource;
@property(nonatomic) uint32_t closeAbsorbed;
@property(nonatomic) BOOL pixelChanged;
@property(nonatomic) uint32_t pixelWidth;
@property(nonatomic) uint32_t pixelHeight;
@property(nonatomic) uint32_t knownWidth;
@property(nonatomic) uint32_t knownHeight;
@property(nonatomic) BOOL needsDisplay;
@property(nonatomic) BOOL minimizedPending;
@property(nonatomic) BOOL restoredPending;
- (BOOL)isPending;
- (void)recordClose:(int32_t)source;
- (void)recordNeedsDisplay;
- (void)recordMinimized;
- (void)recordRestored;
/* A key-down (repeat: the system's auto-repeat flag) or key-up for a macOS virtual key code; a code outside the key
 * table is dropped. */
- (void)recordKeyCode:(uint16_t)code down:(BOOL)down repeat:(BOOL)repeat;
/* A modifier change for the modifier key code: the side is decided from the event's device-dependent side bits. */
- (void)recordFlagsChangedCode:(uint16_t)code flags:(NSUInteger)flags;
/* The window resigned / became key: a loss clears all key state and is delivered before anything recorded after. */
- (void)recordFocusLost;
- (void)recordFocusGained;
/* Fills `out` with at most `cap` key and focus entries (and the resync after an overflow), in recorded order; the
 * pump calls it before dispatch, so input recorded while it runs waits for the next pump. */
- (void)takeKeysInto:(CKWinEvents *)out cap:(size_t)cap;
- (void)nativeKeys:(CKWinNativeKeys *)out;
/* The diagnostic count of discarded transition records so far. */
- (uint64_t)overflowTotal;
/* Updates the layer's contentsScale and records the pixel size if it changed. */
- (void)applyScaleAndRecordPixelSize;
- (double)scale;
- (void)destroyWindow;
@end

#endif

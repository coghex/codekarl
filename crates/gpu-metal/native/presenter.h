#ifndef CK_METAL_PRESENTER_H
#define CK_METAL_PRESENTER_H
#include <Availability.h>
#include <stddef.h>
#include <stdint.h>
#include "textures.h"

/* The Metal presenter's native half. A CKPrCore holds every GPU object the presenter owns (the layer
 * retain, the device, the command queue, the command buffer still referenced, the drawable and
 * command-buffer tokens, and the textured quad's pipeline, samplers and at most two textures); it is
 * created on the main thread and used on the render thread only. Nothing here releases a GPU object
 * except ck_pr_release_all, and the retirement of a replaced quad texture once its token arrived. */
typedef struct CKPrCore CKPrCore;
/* A reference-counted record of what the presenter did, for the tests. Its counters are atomics, so any
 * thread may read them, and it outlives every callback that holds it. */
typedef struct CKPrObserver CKPrObserver;

typedef enum {
    CK_PR_FRAME_DRAWN = 0,     /* the frame was committed */
    CK_PR_FRAME_SKIPPED = 1,   /* nextDrawable returned nil */
    CK_PR_FRAME_BUSY = 2,      /* the previous command buffer, or the drawable tokens, are outstanding */
    CK_PR_FRAME_ABANDONED = 3, /* a drawable was acquired and given back without drawing (test fault) */
    CK_PR_FRAME_FAILED = 4     /* a resource was missing after a drawable was acquired */
} CKPrFrame;

/* Faults injected into one frame step. The presenter's normal start passes none (all zero). */
typedef struct {
    int32_t abandon_after_acquire;
    int32_t nil_command_buffer;
    int32_t nil_render_pass;
    int32_t nil_encoder;
    int32_t partial_encoding;
    int32_t error_completion;
    int32_t never_complete_drawable_tokens; /* the presented handler never sets its token (lost callback) */
    int32_t never_complete_command_tokens;  /* the completion handler never sets its token */
    int32_t handler_delay_ms;         /* the handlers linger this long after setting their tokens */
    int32_t completion_delay_ms;      /* the completion handler waits this long before setting its token */
    int32_t fail_texture_allocation;  /* the quad texture cannot be allocated */
    int32_t fail_quad_encode;         /* the quad cannot be encoded: the frame is abandoned */
    int32_t retired_token_delay_ms;   /* a texture retired now is held by a token that arrives this much later */
    int32_t pause_after_reap_for_token; /* the frame step waits, right after its first look at a retired
                                           texture, until that texture's pending token has arrived: it places a
                                           completion between the reap and the admission (test) */
    int32_t abandon_after_prepare;    /* the frame is abandoned after its ledger token was prepared and before the
                                         commit: a prepared, never committed command buffer (test) */
    int32_t skip_next_drawable;       /* nextDrawable is not called and the step returns CK_PR_FRAME_SKIPPED (test) */
    int32_t capture;                  /* the frame is also rendered, by the same encoding, into an off-screen shared texture
                                         that the observer keeps once the command buffer completes (test) */
    const _Atomic int32_t *completion_gate; /* when given, a frame's completion handler waits (bounded, 30 s) until it reads
                                         non-zero before it sets its token, so a test decides when a committed frame's
                                         completion may arrive; the pointer stays valid for the process's life (test) */
    const _Atomic int32_t *hold_acquire;    /* when given, the step waits (bounded, 30 s) INSIDE the acquisition, between the
                                         marks t3 and t4, while it reads non-zero: a blocked nextDrawable that adds to
                                         the acquire interval only. Read inside the call only (test) */
    const _Atomic int32_t *hold_after_commit; /* when given, the step waits (bounded, 30 s) after the command buffer's commit
                                         returned and before the mark t5 is read, while it reads non-zero: a render thread
                                         descheduled right after the submission. Read inside the call only (test) */
} CKPrFaults;

/* The seam through which a scene presenter's frame attaches its ledger completion token. `prepare` runs on the
 * render thread once the frame is encoded and before the commit; it returns a token the presenter's own
 * completion handler completes (before the presenter's own token), or NULL with a message. A frame step with no
 * ticket passes NULL for the whole struct. */
typedef struct {
    void *userdata;
    CKMcToken *(*prepare)(void *userdata, char *error, size_t capacity);
} CKPrTicket;

/* The textured quad of one frame. `present` 0 means no quad. The destination is in whole drawable pixels
 * from the top-left corner, the source in image pixels; the texture is `width` x `height` RGBA8
 * premultiplied. All validated by the Rust side. */
typedef struct {
    int32_t present;
    int32_t filter; /* 0 nearest, 1 linear */
    int32_t destination[4]; /* left, top, right, bottom */
    float source[4];        /* left, top, right, bottom */
    uint32_t width;
    uint32_t height;
} CKPrQuad;

/* The pixels of a new texture: given only when the quad's image differs from the resident texture's. */
typedef struct {
    const uint8_t *pixels;
    size_t length;
} CKPrUpload;

/* What a frame step did to the quad texture, whatever its return code. */
typedef struct {
    int32_t uploaded; /* a new texture became the resident one */
    int32_t removed;  /* the resident texture was retired and no new one made */
    /* The frame-timing marks (CLOCK_UPTIME_RAW nanoseconds, `clock_gettime_nsec_np`), zero when the step did not reach them:
     * t3 the step calls nextDrawable, t4 nextDrawable returned, t5 the command buffer's commit returned. */
    uint64_t acquire_begin;
    uint64_t acquire_end;
    uint64_t committed;
    /* The clock read immediately BEFORE the command buffer's commit is called: the drawable cannot be presented before it, which t5
     * (read after commit returns, and so possibly late) does not guarantee. Zero when the step did not commit. */
    uint64_t submitted;
} CKPrQuadResult;

/* One timing result consumed from a token by the render thread, taken out by ck_pr_take_timing. kind 1: a command buffer's GPU
 * time (`a` GPUStartTime and `b` GPUEndTime in nanoseconds of the same clock, zero when not reported); kind 2: a drawable's
 * presented time (`a`, zero when it was never shown). `t5` is the commit mark of the frame the result belongs to. */
typedef struct {
    uint32_t kind;
    uint32_t padding;
    uint64_t t5;
    uint64_t a;
    uint64_t b;
} CKPrTiming;

/* One batch of a scene frame: adjacent instances that share a texture, drawn as one instanced draw. `texture` is the
 * Metal texture (an `id<MTLTexture>`, not retained: the frame's ticket keeps it alive) or NULL for the checker. */
typedef struct {
    void *texture;
    uint32_t first; /* the batch's first instance in the instance buffer */
    uint32_t count;
} CKPrBatch;

/* A scene frame's draw list; its instances are already in the core's instance buffer. */
typedef struct {
    const CKPrBatch *batches;
    uint32_t batch_count;
    int32_t filter; /* 0 nearest, 1 linear: the sampler of every non-checker batch (the checker is always nearest) */
    void *checker;  /* the checker's Metal texture (an `id<MTLTexture>`, not retained), bound by every NULL-texture batch */
    /* The bindless frame (milestone 5, issue 10a): when `table` is not NULL (an `id<MTLBuffer>` of 8-byte MTLResourceID
     * entries, not retained: the frame's ticket pins its version) the batches are RUNS of instances that each select their
     * image by the slot in their record; the table, the instance buffer and both samplers are bound once. The images are
     * resident through the queue's residency set. `table_slots` is its entry count (informational). `use_resources`
     * (`use_count` entries, each an `id<MTLResource>`, not retained) is for the off-screen readback only, which has no
     * queue residency set: they are named to the encoder once. The presenter passes NULL and 0. */
    void *table;
    uint32_t table_slots;
    const void *const *use_resources;
    uint32_t use_count;
} CKPrPacket;

/* The retain the presenter takes on the layer (main thread), and its undoing before a core owns it. */
void *ck_pr_layer_retain(void *layer, CKPrObserver *observer);
void ck_pr_layer_release(void *retained_layer, CKPrObserver *observer);

/* A core that owns `retained_layer` (the reference ck_pr_layer_retain returned). Never fails. */
CKPrCore *ck_pr_core_new(void *retained_layer, CKPrObserver *observer);

/* Test only; the presenter's normal start never calls it. Arms a one-shot pause of `ms` milliseconds, in the
 * entry point `entry` (1 ck_pr_outstanding, 2 ck_pr_frame, 3 ck_pr_poll, 4 ck_pr_tokens_state), right after
 * the command token was read as not done: it places a completion between that read and what the caller
 * does with the answer. Render thread, before the first frame. */
void ck_pr_set_check_pause(CKPrCore *core, int32_t entry, int32_t ms);

/* Render thread. Takes the device and command queue from the shared context (one retained reference to each,
 * released by ck_pr_release_all) and configures the layer. 0 on success; otherwise a message. The context
 * itself stays the caller's: it outlives the core. */
int ck_pr_setup(CKPrCore *core, CKMcContext *context, uint32_t width, uint32_t height, char *error,
                size_t capacity);

/* Render thread. Creates the textured quad's pipeline and two samplers from the compiled quad library
 * (BGRA8 target, blending ONE, ONE_MINUS_SRC_ALPHA, clamp-to-edge). 0 on success; otherwise a message.
 * `fail_pipeline` forces the failure (test). Only a presenter started with a quad slot calls it. */
int ck_pr_quad_setup(CKPrCore *core, const uint8_t *library, size_t length, int32_t fail_pipeline,
                     char *error, size_t capacity);

/* Render thread. One frame step inside its own autorelease pool; never waits for a command buffer. */
CKPrFrame ck_pr_frame(CKPrCore *core, const float rgb[3], uint32_t width, uint32_t height,
                      const CKPrFaults *faults, char *message, size_t capacity);

/* ck_pr_frame with the textured quad. `quad` NULL is ck_pr_frame. Before a drawable is acquired (so a
 * failure leaves none outstanding) the step admits, creates and uploads a replacement texture when `upload`
 * is given (the old resident one is retired with the token of the last command buffer that used it), or
 * retires the resident one when the quad is absent; a retirement is admitted only while no earlier one is
 * still waiting for its token, otherwise the step returns CK_PR_FRAME_BUSY. `result` says what happened. */
CKPrFrame ck_pr_frame_quad(CKPrCore *core, const float rgb[3], uint32_t width, uint32_t height,
                           const CKPrFaults *faults, const CKPrQuad *quad, const CKPrUpload *upload,
                           const CKPrPacket *packet, const CKPrTicket *ticket, CKPrQuadResult *result,
                           char *message, size_t capacity);

/* Render thread, setup. Allocates the core's one shared-storage instance buffer of `bytes` bytes (the scene's
 * instances, 48 bytes each) and reports its measured `allocatedSize` in `allocated`. 0 on success. */
int ck_pr_instance_buffer_setup(CKPrCore *core, uint64_t bytes, uint64_t *allocated, char *error,
                                size_t capacity);
/* The instance buffer's contents (valid until the explicit release), or NULL without one. */
void *ck_pr_instance_buffer_contents(CKPrCore *core);
/* Whether the previous frame's command buffer has completed (nothing is outstanding), looked at without waiting: the
 * proof that the instance buffer is free to write. */
int ck_pr_instance_buffer_free(CKPrCore *core);

/* Render thread. Moves up to `capacity` consumed timing results (see CKPrTiming) out of the core's fixed ring of 64, oldest first,
 * and returns how many; `*lost` is added the results dropped because the ring was full since the last call. Never waits. */
uint32_t ck_pr_take_timing(CKPrCore *core, CKPrTiming *out, uint32_t capacity, uint64_t *lost);

/* Render thread. Looks at completed command buffers without waiting; returns 1 and a message once for
 * a command buffer that completed with an error. A completion's error is latched in the core the first
 * time any call sees its token done (this one, ck_pr_outstanding, ck_pr_frame or ck_pr_tokens_state), so
 * no later call can lose it, and the core draws nothing more once one was latched. */
int ck_pr_poll(CKPrCore *core, char *message, size_t capacity);

/* Render thread. Whether a command buffer completion (not yet arrived) or a drawable token is still
 * outstanding, or a latched completion error has not been taken yet. It drops no command buffer. */
int ck_pr_outstanding(CKPrCore *core);

/* Render thread. Looks, without waiting, at every command buffer completion and every drawable token the
 * teardown needs. Returns 1 when all arrived; otherwise 0 and a description of what is outstanding. The
 * first call starts the teardown: from then on nothing the core owns is dropped (the Rust side waits,
 * against its absolute deadline, by calling this again). */
int ck_pr_tokens_state(CKPrCore *core, char *outstanding, size_t capacity);

/* Render thread. Releases every holder the core still owns (command buffer, queue, device, layer) and
 * the core itself. Only after ck_pr_tokens_state returned 1, or after a setup that created nothing
 * outstanding. */
void ck_pr_release_all(CKPrCore *core);

/* The off-screen clear, read back synchronously on the calling thread: width x height BGRA8 pixels. */
int ck_pr_readback(uint32_t width, uint32_t height, const float rgb[3], uint8_t *pixels,
                   size_t length, char *error, size_t capacity);

/* The same, with a quad drawn over the clear by the presenter's own encoding code, pipeline and samplers.
 * `quad` NULL or not present draws the clear alone; otherwise `image` is the quad's RGBA8 pixels. */
/* The pixels of one texture of an off-screen scene (`readback_scene`): RGBA8, rows top first. */
typedef struct {
    const uint8_t *pixels;
    size_t length;
    uint32_t width;
    uint32_t height;
} CKPrReadbackImage;

/* Renders a clear, an optional legacy quad and a scene (instances already laid out as the instance buffer, 48 bytes each,
 * and its batches) off screen through the presenter's own pipeline, samplers and encoding, and reads the pixels back. A
 * batch's `texture` is 1 + the index into `images`, or NULL for the checker (`checker`); the packet's `filter` selects
 * the sampler. With `table_slots` not zero the scene is drawn BINDLESS: the batches are runs (texture NULL), each instance's
 * slot is 1 + its image's index (0 the checker), and a table of `table_slots` entries (slot 0 the checker, 1.. the images
 * given, the rest the checker) is built, bound once and named resident. 0 on success. */
int ck_pr_readback_scene(uint32_t width, uint32_t height, const float rgb[3], const uint8_t *library,
                         size_t library_length, const CKPrQuad *quad, const uint8_t *quad_image,
                         size_t quad_image_length, const CKPrReadbackImage *images, size_t image_count,
                         const CKPrReadbackImage *checker, const void *instances, size_t instance_bytes,
                         const CKPrBatch *batches, uint32_t batch_count, int32_t filter, uint32_t table_slots,
                         uint8_t *pixels, size_t length, char *error, size_t capacity)
    API_AVAILABLE(macos(13.0));

int ck_pr_readback_quad(uint32_t width, uint32_t height, const float rgb[3], const uint8_t *library,
                        size_t library_length, const CKPrQuad *quad, const uint8_t *image,
                        size_t image_length, uint8_t *pixels, size_t length, char *error,
                        size_t capacity);

CKPrObserver *ck_pr_observer_new(void);
/* A captured frame (see `capture`): the `which`-th to complete (0-based), or the latest for -1. Copies its BGRA8 pixels
 * into `pixels` (`length` bytes, exactly width x height x 4), reports its size, and returns how many captures completed
 * so far; -1 when there is none, -2 (with the size set) when the buffer does not fit, -3 when it is no longer kept (the
 * last 64 are). */
int64_t ck_pr_observer_capture(const CKPrObserver *observer, int64_t which, uint32_t *width, uint32_t *height,
                               uint8_t *pixels, size_t length);
void ck_pr_observer_free(CKPrObserver *observer);
/* Counters, by index (see the Rust side for the names). */
int64_t ck_pr_observer_get(const CKPrObserver *observer, int32_t counter);
/* Whether the last abandoned drawable still lives (a weak reference read). */
int32_t ck_pr_observer_abandoned_alive(const CKPrObserver *observer);

#endif

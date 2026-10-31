#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <stdatomic.h>
#include <stdio.h>
#include <time.h>
#include <string.h>
#include <unistd.h>
#include "presenter.h"
#include "textures.h"

/* Observer counters, by index (the Rust side names them). */
enum {
    CK_OBS_DRAWABLE_REFS = 0,
    CK_OBS_DRAWABLE_REFS_MAX = 1,
    CK_OBS_DRAWABLES_ACQUIRED = 2,
    CK_OBS_PRESENTED_RUN = 3,
    CK_OBS_COMPLETED_RUN = 4,
    CK_OBS_IN_FLIGHT_MAX = 5,
    CK_OBS_HOLDERS_RELEASED = 6,
    CK_OBS_LAST_WIDTH = 7,
    CK_OBS_LAST_HEIGHT = 8,
    CK_OBS_STEPS = 9,
    CK_OBS_LATE_HANDLERS = 10,
    CK_OBS_COMMITTED = 11,
    CK_OBS_TOKENS_MAX = 12,
    CK_OBS_LAYER_RETAINS = 13,
    CK_OBS_LAYER_RELEASES = 14,
    CK_OBS_IN_FLIGHT_NOW = 15,
    CK_OBS_BUFFER_HOLDS = 16, /* command buffers the core took hold of */
    CK_OBS_BUFFER_DROPS = 17, /* ... and let go of, however (replacement or the explicit release) */
    CK_OBS_QUAD_TEXTURES_CREATED = 18,  /* quad textures allocated and filled */
    CK_OBS_QUAD_TEXTURES_ALIVE = 19,    /* quad textures a holder of the core still has */
    CK_OBS_QUAD_TEXTURES_MAX = 20,      /* the most alive at once */
    CK_OBS_QUAD_TEXTURES_RELEASED = 21, /* quad textures a holder let go of */
    CK_OBS_QUAD_RELEASED_EARLY = 22,    /* ... of which before the token of their last command buffer */
    CK_OBS_QUAD_RETIRED = 23,           /* resident textures retired (replaced or removed) */
    CK_OBS_QUAD_ADMISSION_WAITS = 24,   /* frame steps that waited for a retired texture's token */
    CK_OBS_QUAD_HOLDERS_RELEASED = 25,  /* textures, pipeline and samplers the explicit release let go of */
    CK_OBS_QUAD_DRAWS = 26,             /* command buffers committed with a quad in them */
    CK_OBS_QUAD_LAST_WIDTH = 27,        /* the size of the texture made last, in pixels */
    CK_OBS_QUAD_LAST_HEIGHT = 28,
    CK_OBS_DEVICE_ID = 29,              /* the identity of the device the core took from the context (test) */
    CK_OBS_QUEUE_ID = 30,               /* ... and of its command queue (test) */
    CK_OBS_CAPTURES = 31,               /* frames captured off screen whose command buffer completed (test) */
    CK_OBS_COUNT = 32
};

/* The entry points a test may pause in (see ck_pr_set_check_pause). */
enum { CK_CAPTURE_KEEP = 64 };

enum {
    CK_ENTRY_NONE = 0,
    CK_ENTRY_OUTSTANDING = 1,
    CK_ENTRY_FRAME = 2,
    CK_ENTRY_POLL = 3,
    CK_ENTRY_TOKENS_STATE = 4
};

/* ---------------------------------------------------------------------------------------------
 * The observer and the tokens: reference-counted objects, independent of the presenter
 * ------------------------------------------------------------------------------------------- */

@interface CKPrObserverState : NSObject {
@public
    _Atomic int64_t counters[CK_OBS_COUNT];
    _Atomic bool released;
}
@property(atomic, weak) id abandoned;
/* The captured frames (test): shared off-screen textures appended by the completion handler of their command buffer, the
 * oldest dropped past CK_CAPTURE_KEEP. `capturedBase` is how many were dropped. Guarded by @synchronized(self). */
@property(nonatomic, strong) NSMutableArray<id<MTLTexture>> *captured;
@property(nonatomic) int64_t capturedBase;
@end

@implementation CKPrObserverState
@end

static void ck_obs_add(CKPrObserverState *observer, int index, int64_t delta) {
    if (observer != nil) {
        atomic_fetch_add(&observer->counters[index], delta);
    }
}

static void ck_obs_max(CKPrObserverState *observer, int index, int64_t value) {
    if (observer == nil) {
        return;
    }
    int64_t seen = atomic_load(&observer->counters[index]);
    while (value > seen &&
           !atomic_compare_exchange_weak(&observer->counters[index], &seen, value)) {
    }
}

/* A token: set by one system callback, read by the render thread. A callback captures its token (and
 * the observer), never a drawable, a command buffer or the presenter, so a late callback can always
 * touch it safely. */
@interface CKPrTokenState : NSObject {
@public
    _Atomic bool done;
    /* Frame timing: what the handler recorded (a command token: GPUStartTime and GPUEndTime; a drawable token: presentedTime in `a`),
     * and the commit mark of the frame, stored by the render thread right after the commit. Handlers only record. */
    _Atomic uint64_t timingA;
    _Atomic uint64_t timingB;
    _Atomic uint64_t commitMark;
}
@property(atomic, copy) NSString *error;
@property(nonatomic, copy) NSString *label;
/* Render thread only: this token's error (if any) was already taken into the core. */
@property(nonatomic) BOOL latched;
/* Render thread only: a drawable token's timing result was already moved into the core's ring. */
@property(nonatomic) BOOL timingTaken;
@end

@implementation CKPrTokenState
@end

/* ---------------------------------------------------------------------------------------------
 * The core
 * ------------------------------------------------------------------------------------------- */

/* The textured quad's pipeline and its two samplers: holders of the core, released only by the teardown. */
@interface CKPrQuadKit : NSObject
@property(nonatomic, strong) id<MTLRenderPipelineState> pipeline;
/* The instanced scene pipeline: the same blending and target, the vertex function that reads the instance buffer. */
@property(nonatomic, strong) id<MTLRenderPipelineState> instanced;
/* The bindless scene pipeline: the same blending and target; the vertex function passes each instance's slot on, the fragment
 * function selects the image from the frame's table. */
@property(nonatomic, strong) id<MTLRenderPipelineState> bindless;
@property(nonatomic, strong) id<MTLSamplerState> nearest;
@property(nonatomic, strong) id<MTLSamplerState> linear;
@end

@implementation CKPrQuadKit
@end

/* The ring of consumed timing results: filled by the render thread when it consumes a token, drained by ck_pr_take_timing. */
#define CK_TIMING_RING 64

@interface CKPrCoreState : NSObject {
@public
    CKPrTiming timing[CK_TIMING_RING];
    uint32_t timingHead;
    uint32_t timingCount;
    uint64_t timingLost;
}
@property(nonatomic, strong) CAMetalLayer *layer;
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) id<MTLCommandQueue> queue;
/* The scene's one shared-storage instance buffer (set once by ck_pr_instance_buffer_setup): written by the render thread
 * only after the previous frame's command buffer completed, read by the GPU until this frame's completes. */
@property(nonatomic, strong) id<MTLBuffer> instanceBuffer;
/* The last committed command buffer and its token, kept until the next command buffer replaces them
 * or the explicit release lets them go; never dropped by a look at the tokens. */
@property(nonatomic, strong) id<MTLCommandBuffer> buffer;
@property(nonatomic, strong) CKPrTokenState *bufferToken;
@property(nonatomic, strong) NSMutableArray<CKPrTokenState *> *drawableTokens;
@property(nonatomic, strong) CKPrObserverState *observer;
/* The textured quad: its pipeline and samplers (set once by ck_pr_quad_setup), the resident texture, and at
 * most one retired texture still waiting for the token of the last command buffer that used it. At most two
 * quad textures are alive. */
@property(nonatomic, strong) CKPrQuadKit *quadKit;
@property(nonatomic, strong) id<MTLTexture> quadTexture;
@property(nonatomic, strong) id<MTLTexture> retiredTexture;
@property(nonatomic, strong) CKPrTokenState *retiredToken;
/* A completion error, latched when first seen and not yet handed out; and that one was seen at all. */
@property(nonatomic, copy) NSString *latchedError;
@property(nonatomic) BOOL failedSticky;
/* The teardown has begun: nothing the core owns is dropped any more. */
@property(nonatomic) BOOL tearingDown;
/* Test only (zero in production): the entry point now running, the one where a test wants a pause right
 * after the command token was read as not done, and for how long (disarmed after it fires). */
@property(nonatomic) int32_t entry;
@property(nonatomic) int32_t pauseEntry;
@property(nonatomic) int32_t pauseMs;
@end

/* Every change of the command buffer holder, by whatever code, goes through this setter, so the observer
 * sees the real transitions (a replacement counts as a drop and a hold). */
@implementation CKPrCoreState {
    id<MTLCommandBuffer> _buffer;
}

- (id<MTLCommandBuffer>)buffer {
    return _buffer;
}

- (void)setBuffer:(id<MTLCommandBuffer>)buffer {
    if (_buffer != nil) {
        ck_obs_add(self.observer, CK_OBS_BUFFER_DROPS, 1);
    }
    if (buffer != nil) {
        ck_obs_add(self.observer, CK_OBS_BUFFER_HOLDS, 1);
    }
    _buffer = buffer;
}
@end

static void ck_message(char *buffer, size_t capacity, NSString *text) {
    if (buffer != NULL && capacity != 0) {
        snprintf(buffer, capacity, "%s", text.UTF8String ?: "unknown presenter error");
    }
}

/* The quad's uniforms, as the shader reads them (48 bytes). */
typedef struct {
    float destination[4];
    float source[4];
    float viewport[2];
    float padding[2];
} CKPrQuadUniforms;

/* Draws the quad into the encoder's render pass: the pipeline (blending ONE, ONE_MINUS_SRC_ALPHA), the four
 * vertices of a strip, the sampler for the quad's filter. The destination is in drawable pixels and the
 * source is divided by the image size here. */
static void ck_encode_quad(id<MTLRenderCommandEncoder> encoder, CKPrQuadKit *kit, id<MTLTexture> image,
                           const CKPrQuad *quad, NSUInteger target_width, NSUInteger target_height) {
    CKPrQuadUniforms uniforms;
    for (int index = 0; index < 4; index++) {
        uniforms.destination[index] = (float)quad->destination[index];
        float extent = index % 2 == 0 ? (float)quad->width : (float)quad->height;
        uniforms.source[index] = quad->source[index] / extent;
    }
    uniforms.viewport[0] = (float)target_width;
    uniforms.viewport[1] = (float)target_height;
    uniforms.padding[0] = 0;
    uniforms.padding[1] = 0;
    [encoder setRenderPipelineState:kit.pipeline];
    [encoder setVertexBytes:&uniforms length:sizeof(uniforms) atIndex:0];
    [encoder setFragmentTexture:image atIndex:0];
    [encoder setFragmentSamplerState:(quad->filter != 0 ? kit.linear : kit.nearest) atIndex:0];
    [encoder drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4];
}

/* The instance data the instanced vertex function reads for a batch (`InstanceUniforms` in quad.metal). */
typedef struct {
    float viewport[2];
    uint32_t first;
    uint32_t padding;
} CKPrInstanceUniforms;

/* Draws a scene frame's batches into the encoder's render pass, in order, over what is already there: one instanced draw
 * per batch (its texture and sampler bound, its first instance passed explicitly). The checker (a NULL texture) always
 * uses the nearest sampler so its cells stay sharp; the other batches use the frame's filter. */
static void ck_encode_scene(id<MTLRenderCommandEncoder> encoder, CKPrQuadKit *kit, const CKPrPacket *packet,
                            id<MTLBuffer> instances, NSUInteger target_width, NSUInteger target_height) {
    if (packet->table != NULL) {
        /* The bindless frame: the pinned table, the instance buffer and both samplers are bound once, then one instanced draw
         * per run (a run is the whole scene unless a named limit splits it). The images are resident through the queue's
         * residency set (an off-screen test names its textures in `use_resources`), never through a per-frame loop here. */
        if (packet->use_resources != NULL && packet->use_count != 0) {
            __unsafe_unretained id<MTLResource> *listed =
                (__unsafe_unretained id<MTLResource> *)calloc(packet->use_count, sizeof(id<MTLResource>));
            if (listed != NULL) {
                for (uint32_t index = 0; index < packet->use_count; index++) {
                    listed[index] = (__bridge id<MTLResource>)packet->use_resources[index];
                }
                [encoder useResources:listed count:packet->use_count usage:MTLResourceUsageRead
                               stages:MTLRenderStageFragment];
                free(listed);
            }
        }
        [encoder setRenderPipelineState:kit.bindless];
        [encoder setVertexBuffer:instances offset:0 atIndex:1];
        [encoder setFragmentBuffer:(__bridge id<MTLBuffer>)packet->table offset:0 atIndex:0];
        [encoder setFragmentSamplerState:kit.nearest atIndex:0];
        [encoder setFragmentSamplerState:(packet->filter != 0 ? kit.linear : kit.nearest) atIndex:1];
        for (uint32_t index = 0; index < packet->batch_count; index++) {
            const CKPrBatch *run = &packet->batches[index];
            CKPrInstanceUniforms uniforms = {{(float)target_width, (float)target_height}, run->first, 0};
            [encoder setVertexBytes:&uniforms length:sizeof(uniforms) atIndex:0];
            [encoder drawPrimitives:MTLPrimitiveTypeTriangleStrip
                        vertexStart:0
                        vertexCount:4
                      instanceCount:run->count];
        }
        return;
    }
    id<MTLTexture> checker = packet->checker != NULL ? (__bridge id<MTLTexture>)packet->checker : nil;
    [encoder setRenderPipelineState:kit.instanced];
    [encoder setVertexBuffer:instances offset:0 atIndex:1];
    for (uint32_t index = 0; index < packet->batch_count; index++) {
        const CKPrBatch *batch = &packet->batches[index];
        id<MTLTexture> image = batch->texture != NULL ? (__bridge id<MTLTexture>)batch->texture : checker;
        if (image == nil) {
            continue;
        }
        CKPrInstanceUniforms uniforms = {{(float)target_width, (float)target_height}, batch->first, 0};
        [encoder setVertexBytes:&uniforms length:sizeof(uniforms) atIndex:0];
        [encoder setFragmentTexture:image atIndex:0];
        [encoder setFragmentSamplerState:(batch->texture != NULL && packet->filter != 0 ? kit.linear : kit.nearest)
                                 atIndex:0];
        [encoder drawPrimitives:MTLPrimitiveTypeTriangleStrip
                    vertexStart:0
                    vertexCount:4
                  instanceCount:batch->count];
    }
}

/* The frame, encoded the same way for a drawable and for the off-screen texture: the clear and, if `quad`
 * is present, the quad over it in the same render pass, then the scene's batches over both (painter order: the legacy
 * quad is below). Returns the command buffer, or nil with a reason. */
static id<MTLCommandBuffer> ck_encode_frame(id<MTLCommandQueue> queue, id<MTLTexture> texture,
                                            const float rgb[3], const CKPrFaults *faults,
                                            CKPrQuadKit *kit, id<MTLTexture> image, const CKPrQuad *quad,
                                            const CKPrPacket *packet, id<MTLBuffer> instances,
                                            id<MTLTexture> capture, const CKPrTicket *ticket,
                                            CKMcToken **ledger_token, NSString **reason, BOOL *partial) {
    id<MTLCommandBuffer> command = faults != NULL && faults->nil_command_buffer ? nil : [queue commandBuffer];
    if (command == nil) {
        *reason = @"no command buffer could be created";
        return nil;
    }
    MTLRenderPassDescriptor *pass = faults != NULL && faults->nil_render_pass
                                        ? nil
                                        : [MTLRenderPassDescriptor renderPassDescriptor];
    if (pass == nil) {
        *reason = @"no render pass descriptor could be created";
        return nil;
    }
    pass.colorAttachments[0].texture = texture;
    pass.colorAttachments[0].loadAction = MTLLoadActionClear;
    pass.colorAttachments[0].storeAction = MTLStoreActionStore;
    pass.colorAttachments[0].clearColor = MTLClearColorMake(rgb[0], rgb[1], rgb[2], 1.0);
    id<MTLRenderCommandEncoder> encoder = faults != NULL && faults->nil_encoder
                                              ? nil
                                              : [command renderCommandEncoderWithDescriptor:pass];
    if (encoder == nil) {
        *reason = @"no render command encoder could be created";
        return nil;
    }
    BOOL drawing = quad != NULL && quad->present != 0;
    if (drawing && (kit == nil || image == nil || (faults != NULL && faults->fail_quad_encode))) {
        [encoder endEncoding];
        *partial = YES;
        *reason = @"the quad could not be encoded";
        return nil;
    }
    BOOL scene = packet != NULL && packet->batch_count != 0;
    if (scene && (kit == nil || kit.instanced == nil || instances == nil || packet->checker == NULL ||
                  (faults != NULL && faults->fail_quad_encode))) {
        [encoder endEncoding];
        *partial = YES;
        *reason = @"the scene could not be encoded";
        return nil;
    }
    if (drawing) {
        ck_encode_quad(encoder, kit, image, quad, texture.width, texture.height);
    }
    if (scene) {
        ck_encode_scene(encoder, kit, packet, instances, texture.width, texture.height);
    }
    [encoder endEncoding];
    if (capture != nil) {
        /* Test only: the same clear, quad and scene, encoded again into the off-screen texture. */
        MTLRenderPassDescriptor *again_pass = [MTLRenderPassDescriptor renderPassDescriptor];
        again_pass.colorAttachments[0].texture = capture;
        again_pass.colorAttachments[0].loadAction = MTLLoadActionClear;
        again_pass.colorAttachments[0].storeAction = MTLStoreActionStore;
        again_pass.colorAttachments[0].clearColor = MTLClearColorMake(rgb[0], rgb[1], rgb[2], 1.0);
        id<MTLRenderCommandEncoder> again = [command renderCommandEncoderWithDescriptor:again_pass];
        if (again == nil) {
            *reason = @"no render command encoder could be created for the capture";
            return nil;
        }
        if (drawing) {
            ck_encode_quad(again, kit, image, quad, capture.width, capture.height);
        }
        if (scene) {
            ck_encode_scene(again, kit, packet, instances, capture.width, capture.height);
        }
        [again endEncoding];
    }
    if (ticket != NULL) {
        /* The ledger's completion token for this command buffer, before the commit. A refusal abandons the
         * frame; once it succeeded the frame is prepared, and a later abandonment is "not committed". */
        char prepare_error[256] = "";
        CKMcToken *token = ticket->prepare(ticket->userdata, prepare_error, sizeof prepare_error);
        if (token == NULL) {
            *reason = [NSString stringWithFormat:@"the frame's completion token could not be prepared: %s",
                                                 prepare_error];
            return nil;
        }
        *ledger_token = token;
    }
    if (faults != NULL && faults->partial_encoding) {
        /* An encoder was started and ended, and the work is abandoned before it is committed. */
        *partial = YES;
        *reason = @"the frame was abandoned after partial encoding";
        return nil;
    }
    if (faults != NULL && faults->abandon_after_prepare) {
        *reason = @"the frame was abandoned after its token was prepared";
        return nil;
    }
    return command;
}

void *ck_pr_layer_retain(void *layer, CKPrObserver *observer) {
    ck_obs_add((__bridge CKPrObserverState *)observer, CK_OBS_LAYER_RETAINS, 1);
    return (void *)CFBridgingRetain((__bridge id)layer);
}

void ck_pr_layer_release(void *retained_layer, CKPrObserver *observer) {
    if (retained_layer != NULL) {
        ck_obs_add((__bridge CKPrObserverState *)observer, CK_OBS_LAYER_RELEASES, 1);
        __unused id layer = CFBridgingRelease(retained_layer);
    }
}

CKPrCore *ck_pr_core_new(void *retained_layer, CKPrObserver *observer) {
    CKPrCoreState *core = [CKPrCoreState new];
    core.layer = (CAMetalLayer *)CFBridgingRelease(retained_layer);
    core.drawableTokens = [NSMutableArray array];
    core.observer = (__bridge CKPrObserverState *)observer;
    return (__bridge_retained CKPrCore *)core;
}

void ck_pr_set_check_pause(CKPrCore *handle, int32_t entry, int32_t ms) {
    CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
    core.pauseEntry = entry;
    core.pauseMs = ms;
}

int ck_pr_setup(CKPrCore *handle, CKMcContext *context, uint32_t width, uint32_t height, char *error,
                size_t capacity) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        id<MTLDevice> device = (__bridge_transfer id<MTLDevice>)ck_mc_context_device(context);
        id<MTLCommandQueue> queue = (__bridge_transfer id<MTLCommandQueue>)ck_mc_context_queue(context);
        if (device == nil) {
            ck_message(error, capacity, @"No Metal device is available");
            return 1;
        }
        if (queue == nil) {
            ck_message(error, capacity, @"the Metal command queue could not be created");
            return 1;
        }
        core.device = device;
        core.queue = queue;
        if (core.observer != nil) {
            /* What the core now holds, not what was taken: a core that made its own would show it. */
            atomic_store(&core.observer->counters[CK_OBS_DEVICE_ID], (int64_t)(intptr_t)(__bridge void *)core.device);
            atomic_store(&core.observer->counters[CK_OBS_QUEUE_ID], (int64_t)(intptr_t)(__bridge void *)core.queue);
        }
        core.layer.device = device;
        core.layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
        core.layer.framebufferOnly = YES;
        core.layer.maximumDrawableCount = 3;
        core.layer.allowsNextDrawableTimeout = YES;
        core.layer.drawableSize = CGSizeMake(width, height);
        return 0;
    }
}

/* The instrumentation's one clock: CLOCK_UPTIME_RAW nanoseconds. A CFTimeInterval host time (presentedTime, GPUStartTime) is seconds
 * of mach_absolute_time scaled by the host time base, which is this clock, so the conversion is a scaling (zero for no time). */
static uint64_t ck_now_ns(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

static uint64_t ck_seconds_ns(double seconds) {
    return (isfinite(seconds) && seconds > 0.0) ? (uint64_t)llround(seconds * 1.0e9) : 0;
}

/* A consumed timing result goes into the core's fixed ring (render thread only); a full ring drops the new result and counts it. */
static void ck_push_timing(CKPrCoreState *core, uint32_t kind, uint64_t commit_mark, uint64_t a, uint64_t b) {
    if (commit_mark == 0) {
        return; /* a token of a frame the instrumentation did not measure (abandoned before its commit) */
    }
    if (core->timingCount >= CK_TIMING_RING) {
        core->timingLost += 1;
        return;
    }
    uint32_t slot = (core->timingHead + core->timingCount) % CK_TIMING_RING;
    core->timing[slot] = (CKPrTiming){.kind = kind, .padding = 0, .t5 = commit_mark, .a = a, .b = b};
    core->timingCount += 1;
}

/* The one reader of the command token's done flag. Whether the last command buffer's completion has not
 * arrived: YES when this single read of the flag says so; NO when it says the completion arrived, and
 * then, in this same step, the token's error is taken into the core (once). Every decision that depends on
 * the completion (a timer-less wait, a new frame replacing the token) uses this result and never reads the
 * flag again, so a completion that lands after this read cannot authorise anything. */
static BOOL ck_command_outstanding(CKPrCoreState *core) {
    CKPrTokenState *token = core.bufferToken;
    if (token == nil || token.latched) {
        return NO;
    }
    if (!atomic_load(&token->done)) {
        if (core.pauseMs > 0 && core.pauseEntry == core.entry) {
            /* Test only: the thread is descheduled here, after the read. */
            int32_t pause = core.pauseMs;
            core.pauseMs = 0;
            usleep((useconds_t)pause * 1000);
        }
        return YES;
    }
    token.latched = YES;
    ck_push_timing(core, 1, atomic_load(&token->commitMark), atomic_load(&token->timingA), atomic_load(&token->timingB));
    NSString *error = token.error;
    if (error != nil) {
        core.failedSticky = YES;
        if (core.latchedError == nil) {
            core.latchedError = error;
        }
    }
    return NO;
}

/* The drawable tokens whose handler has not run. */
static NSUInteger ck_drawables_outstanding(CKPrCoreState *core) {
    NSUInteger count = 0;
    for (CKPrTokenState *token in core.drawableTokens) {
        if (!atomic_load(&token->done)) {
            count += 1;
        }
    }
    return count;
}

/* Moves the timing result of every drawable token that arrived into the ring, once (also during the teardown, where the tokens are
 * kept for the proof), and drops the drawable tokens that arrived (not once the teardown began). It never drops the command buffer
 * or its token. */
static void ck_prune_drawables(CKPrCoreState *core) {
    NSMutableArray<CKPrTokenState *> *kept = [NSMutableArray array];
    for (CKPrTokenState *token in core.drawableTokens) {
        BOOL arrived = atomic_load(&token->done);
        if (arrived && !token.timingTaken) {
            token.timingTaken = YES;
            ck_push_timing(core, 2, atomic_load(&token->commitMark), atomic_load(&token->timingA), 0);
        }
        if (!arrived || core.tearingDown) {
            [kept addObject:token];
        }
    }
    core.drawableTokens = kept;
}

/* Hands out the latched error, once. */
static BOOL ck_take_error(CKPrCoreState *core, char *message, size_t capacity) {
    NSString *error = core.latchedError;
    if (error == nil) {
        return NO;
    }
    core.latchedError = nil;
    ck_message(message, capacity, error);
    return YES;
}

/* The core's explicit let-go of its command buffer (the next commit replaces it through the same setter). */
static void ck_drop_buffer(CKPrCoreState *core) {
    core.buffer = nil;
    core.bufferToken = nil;
}

/* ---------------------------------------------------------------------------------------------
 * The textured quad: pipeline, samplers, textures
 * ------------------------------------------------------------------------------------------- */

/* The pipeline and samplers from the compiled quad library, for a BGRA8 target. nil with a reason. */
static CKPrQuadKit *ck_make_quad_kit(id<MTLDevice> device, const uint8_t *library_bytes, size_t length,
                                     int32_t fail, NSString **reason) {
    if (fail) {
        *reason = @"the quad pipeline could not be created (forced)";
        return nil;
    }
    NSError *failure = nil;
    dispatch_data_t data = dispatch_data_create(library_bytes, length,
        dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), DISPATCH_DATA_DESTRUCTOR_DEFAULT);
    id<MTLLibrary> library = [device newLibraryWithData:data error:&failure];
    id<MTLFunction> vertex = [library newFunctionWithName:@"ck_quad_vertex"];
    id<MTLFunction> fragment = [library newFunctionWithName:@"ck_quad_fragment"];
    id<MTLFunction> instance_vertex = [library newFunctionWithName:@"ck_quad_instance_vertex"];
    id<MTLFunction> bindless_vertex = [library newFunctionWithName:@"ck_quad_bindless_vertex"];
    id<MTLFunction> bindless_fragment = [library newFunctionWithName:@"ck_quad_bindless_fragment"];
    if (library == nil || vertex == nil || fragment == nil || instance_vertex == nil || bindless_vertex == nil ||
        bindless_fragment == nil) {
        *reason = [NSString stringWithFormat:@"the quad shader library could not be loaded: %@",
                                             failure.localizedDescription ?: @"a function is missing"];
        return nil;
    }
    MTLRenderPipelineDescriptor *description = [MTLRenderPipelineDescriptor new];
    description.vertexFunction = vertex;
    description.fragmentFunction = fragment;
    MTLRenderPipelineColorAttachmentDescriptor *color = description.colorAttachments[0];
    color.pixelFormat = MTLPixelFormatBGRA8Unorm;
    color.blendingEnabled = YES;
    color.rgbBlendOperation = MTLBlendOperationAdd;
    color.alphaBlendOperation = MTLBlendOperationAdd;
    color.sourceRGBBlendFactor = MTLBlendFactorOne;
    color.sourceAlphaBlendFactor = MTLBlendFactorOne;
    color.destinationRGBBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
    color.destinationAlphaBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
    id<MTLRenderPipelineState> pipeline = [device newRenderPipelineStateWithDescriptor:description
                                                                                  error:&failure];
    if (pipeline == nil) {
        *reason = [NSString stringWithFormat:@"the quad pipeline could not be created: %@",
                                             failure.localizedDescription ?: @"unknown error"];
        return nil;
    }
    description.vertexFunction = instance_vertex;
    id<MTLRenderPipelineState> instanced = [device newRenderPipelineStateWithDescriptor:description
                                                                                  error:&failure];
    if (instanced == nil) {
        *reason = [NSString stringWithFormat:@"the instanced quad pipeline could not be created: %@",
                                             failure.localizedDescription ?: @"unknown error"];
        return nil;
    }
    description.vertexFunction = bindless_vertex;
    description.fragmentFunction = bindless_fragment;
    id<MTLRenderPipelineState> bindless = [device newRenderPipelineStateWithDescriptor:description error:&failure];
    if (bindless == nil) {
        *reason = [NSString stringWithFormat:@"the bindless quad pipeline could not be created: %@",
                                             failure.localizedDescription ?: @"unknown error"];
        return nil;
    }
    CKPrQuadKit *kit = [CKPrQuadKit new];
    kit.pipeline = pipeline;
    kit.instanced = instanced;
    kit.bindless = bindless;
    for (int linear = 0; linear < 2; linear++) {
        MTLSamplerDescriptor *sampler = [MTLSamplerDescriptor new];
        MTLSamplerMinMagFilter filter = linear ? MTLSamplerMinMagFilterLinear : MTLSamplerMinMagFilterNearest;
        sampler.minFilter = filter;
        sampler.magFilter = filter;
        sampler.mipFilter = MTLSamplerMipFilterNotMipmapped;
        sampler.sAddressMode = MTLSamplerAddressModeClampToEdge;
        sampler.tAddressMode = MTLSamplerAddressModeClampToEdge;
        sampler.normalizedCoordinates = YES;
        id<MTLSamplerState> state = [device newSamplerStateWithDescriptor:sampler];
        if (state == nil) {
            *reason = @"a quad sampler could not be created";
            return nil;
        }
        if (linear) {
            kit.linear = state;
        } else {
            kit.nearest = state;
        }
    }
    return kit;
}

/* A shared-storage RGBA8 texture of the image's size with its pixels copied in (the CPU copy completes
 * before the frame that samples it is committed). nil with a reason naming the size. */
static id<MTLTexture> ck_make_quad_texture(id<MTLDevice> device, uint32_t width, uint32_t height,
                                           const CKPrUpload *upload, int32_t fail, NSString **reason) {
    size_t bytes = (size_t)width * height * 4;
    if (width == 0 || height == 0 || upload == NULL || upload->length != bytes) {
        *reason = [NSString stringWithFormat:@"the pixels do not fill a %u x %u quad texture", width, height];
        return nil;
    }
    MTLTextureDescriptor *description =
        [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
                                                           width:width
                                                          height:height
                                                       mipmapped:NO];
    description.usage = MTLTextureUsageShaderRead;
    description.storageMode = MTLStorageModeShared;
    id<MTLTexture> texture = fail ? nil : [device newTextureWithDescriptor:description];
    if (texture == nil) {
        *reason = [NSString stringWithFormat:@"a quad texture of %u x %u pixels (%zu bytes) could not be allocated",
                                             width, height, bytes];
        return nil;
    }
    [texture replaceRegion:MTLRegionMake2D(0, 0, width, height)
               mipmapLevel:0
                 withBytes:upload->pixels
               bytesPerRow:(NSUInteger)width * 4];
    return texture;
}

/* Lets go of the retired texture once the token of the last command buffer that used it has arrived; never
 * once the teardown began, and never before the token. */
static void ck_quad_reap(CKPrCoreState *core) {
    if (core.tearingDown || core.retiredTexture == nil) {
        return;
    }
    CKPrTokenState *token = core.retiredToken;
    if (token != nil && !atomic_load(&token->done)) {
        return;
    }
    ck_obs_add(core.observer, CK_OBS_QUAD_TEXTURES_RELEASED, 1);
    ck_obs_add(core.observer, CK_OBS_QUAD_TEXTURES_ALIVE, -1);
    core.retiredTexture = nil;
    core.retiredToken = nil;
}

/* Whether a retired texture still waits for its token (then no further texture is admitted). */
static BOOL ck_quad_retired_waiting(CKPrCoreState *core) {
    if (core.retiredTexture == nil) {
        return NO;
    }
    CKPrTokenState *token = core.retiredToken;
    return token != nil && !atomic_load(&token->done);
}

/* Retires the resident texture: it is held, with the token of the last command buffer that used it, until
 * that token arrives. It refuses (NO) while any retired texture is held, whether or not its token has
 * arrived, so a retirement's accounting is never overwritten: the caller reaped first and only retires when
 * nothing is held. `delay_ms` (test) makes the token a stand-in that arrives that much later, as if a command
 * buffer still used the texture. */
static BOOL ck_quad_retire(CKPrCoreState *core, int32_t delay_ms) {
    if (core.retiredTexture != nil) {
        return NO;
    }
    CKPrTokenState *token = core.bufferToken;
    if (delay_ms > 0) {
        CKPrTokenState *late = [CKPrTokenState new];
        late.label = @"a retired texture's stand-in token (test)";
        token = late;
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)delay_ms * NSEC_PER_MSEC),
                       dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
                           atomic_store(&late->done, true);
                       });
    }
    core.retiredTexture = core.quadTexture;
    core.quadTexture = nil;
    core.retiredToken = token;
    ck_obs_add(core.observer, CK_OBS_QUAD_RETIRED, 1);
    return YES;
}

int ck_pr_quad_setup(CKPrCore *handle, const uint8_t *library, size_t length, int32_t fail_pipeline,
                     char *error, size_t capacity) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        NSString *reason = nil;
        CKPrQuadKit *kit = nil;
        if (core.device != nil) {
            kit = ck_make_quad_kit(core.device, library, length, fail_pipeline, &reason);
        }
        if (kit == nil) {
            ck_message(error, capacity, reason ?: @"the quad pipeline needs a device");
            return 1;
        }
        core.quadKit = kit;
        return 0;
    }
}

int ck_pr_instance_buffer_setup(CKPrCore *handle, uint64_t bytes, uint64_t *allocated, char *error,
                                size_t capacity) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        if (core.device == nil || bytes == 0 || bytes > (uint64_t)NSUIntegerMax) {
            ck_message(error, capacity, @"the instance buffer needs a device and a size");
            return 1;
        }
        id<MTLBuffer> buffer = [core.device newBufferWithLength:(NSUInteger)bytes
                                                        options:MTLResourceStorageModeShared];
        if (buffer == nil) {
            ck_message(error, capacity,
                       [NSString stringWithFormat:@"the instance buffer of %llu bytes could not be allocated",
                                                  (unsigned long long)bytes]);
            return 1;
        }
        *allocated = (uint64_t)buffer.allocatedSize;
        core.instanceBuffer = buffer;
        return 0;
    }
}

void *ck_pr_instance_buffer_contents(CKPrCore *handle) {
    CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
    return core.instanceBuffer != nil ? core.instanceBuffer.contents : NULL;
}

int ck_pr_instance_buffer_free(CKPrCore *handle) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        core.entry = CK_ENTRY_FRAME;
        /* One look at the previous command buffer's completion token; a completion error is taken in this same look. */
        return ck_command_outstanding(core) ? 0 : 1;
    }
}

/* A handler: (as a fault, waits, then) sets its token (unless the fault loses the callback), then, as a
 * fault, lingers: a late
 * callback must still touch nothing but its token and the observer, however long after the presenter's
 * end it returns. */
static void ck_set_token(CKPrTokenState *token, CKPrObserverState *observer, int run_counter,
                         int32_t never, int32_t before_ms, int32_t linger_ms, NSString *error,
                         CKMcToken *ledger_token) {
    ck_obs_add(observer, run_counter, 1);
    if (before_ms > 0) {
        usleep((useconds_t)before_ms * 1000);
    }
    if (error != nil) {
        token.error = error;
    }
    if (!never) {
        /* The ledger's token first, so it is never later than this core's own token; nothing touches it after. */
        ck_mc_command_token_complete(ledger_token, error.UTF8String);
        atomic_store(&token->done, true);
    }
    if (linger_ms > 0) {
        usleep((useconds_t)linger_ms * 1000);
    }
    if (observer != nil && atomic_load(&observer->released)) {
        ck_obs_add(observer, CK_OBS_LATE_HANDLERS, 1);
    }
}

CKPrFrame ck_pr_frame(CKPrCore *handle, const float rgb[3], uint32_t width, uint32_t height,
                      const CKPrFaults *faults, char *message, size_t capacity) {
    return ck_pr_frame_quad(handle, rgb, width, height, faults, NULL, NULL, NULL, NULL, NULL, message, capacity);
}

CKPrFrame ck_pr_frame_quad(CKPrCore *handle, const float rgb[3], uint32_t width, uint32_t height,
                           const CKPrFaults *faults, const CKPrQuad *quad, const CKPrUpload *upload,
                           const CKPrPacket *packet, const CKPrTicket *ticket, CKPrQuadResult *result,
                           char *message, size_t capacity) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        CKPrObserverState *observer = core.observer;
        /* `result` is shadowed by the frame code later in this function: the timing marks go through this name. */
        CKPrQuadResult *quad_result = result;
        if (result != NULL) {
            result->uploaded = 0;
            result->removed = 0;
            result->acquire_begin = 0;
            result->acquire_end = 0;
            result->committed = 0;
        }
        ck_obs_add(observer, CK_OBS_STEPS, 1);
        core.entry = CK_ENTRY_FRAME;
        ck_prune_drawables(core);
        ck_quad_reap(core);
        if (faults->pause_after_reap_for_token && core.retiredTexture != nil && core.retiredToken != nil) {
            /* Test only: the token arrives between the reap above and the admission below. Bounded at 5 s. */
            for (int waited = 0; waited < 5000 && !atomic_load(&core.retiredToken->done); waited++) {
                usleep(1000);
            }
        }
        /* One look at the previous command buffer: if it completed, its error is taken in this same step. */
        BOOL commandOutstanding = ck_command_outstanding(core);
        /* An earlier command buffer failed: report it and draw nothing more. */
        if (core.failedSticky) {
            if (!ck_take_error(core, message, capacity)) {
                ck_message(message, capacity, @"an earlier command buffer failed");
            }
            return CK_PR_FRAME_FAILED;
        }
        /* One frame in flight, checked without waiting; at most maximumDrawableCount drawable tokens. */
        if (commandOutstanding ||
            core.drawableTokens.count >= core.layer.maximumDrawableCount) {
            return CK_PR_FRAME_BUSY;
        }
        /* The quad texture, before a drawable is acquired, so that no failure here leaves one outstanding. A
         * texture is replaced or removed only when no retired texture is held at all (the explicit admission:
         * at most one resident and one retired texture are alive). The decision is the reap's own: the reap
         * above released the retired texture if its token had arrived when it looked, and if the texture is
         * still held the step waits, even should the token arrive a moment later (the next step reaps it).
         * The token is never read a second time to decide that the holder is gone. */
        if (quad != NULL) {
            BOOL wanted = quad->present != 0;
            BOOL replace = (wanted && upload != NULL) || (!wanted && core.quadTexture != nil);
            if (replace) {
                if (core.retiredTexture != nil) {
                    ck_obs_add(observer, CK_OBS_QUAD_ADMISSION_WAITS, 1);
                    return CK_PR_FRAME_BUSY;
                }
                id<MTLTexture> fresh = nil;
                if (wanted) {
                    NSString *why = nil;
                    fresh = ck_make_quad_texture(core.device, quad->width, quad->height, upload,
                                                 faults->fail_texture_allocation, &why);
                    if (fresh == nil) {
                        ck_message(message, capacity, why);
                        return CK_PR_FRAME_FAILED;
                    }
                    ck_obs_add(observer, CK_OBS_QUAD_TEXTURES_CREATED, 1);
                    ck_obs_add(observer, CK_OBS_QUAD_TEXTURES_ALIVE, 1);
                    ck_obs_max(observer, CK_OBS_QUAD_TEXTURES_MAX,
                               observer == nil ? 0 : atomic_load(&observer->counters[CK_OBS_QUAD_TEXTURES_ALIVE]));
                    if (observer != nil) {
                        atomic_store(&observer->counters[CK_OBS_QUAD_LAST_WIDTH], (int64_t)quad->width);
                        atomic_store(&observer->counters[CK_OBS_QUAD_LAST_HEIGHT], (int64_t)quad->height);
                    }
                }
                if (core.quadTexture != nil && !ck_quad_retire(core, faults->retired_token_delay_ms)) {
                    ck_message(message, capacity, @"a quad texture was retired while another was still held");
                    return CK_PR_FRAME_FAILED;
                }
                core.quadTexture = fresh;
                if (result != NULL) {
                    result->uploaded = wanted ? 1 : 0;
                    result->removed = wanted ? 0 : 1;
                }
                ck_quad_reap(core);
            } else if (wanted && core.quadTexture == nil) {
                ck_message(message, capacity, @"a quad was drawn with no texture to sample");
                return CK_PR_FRAME_FAILED;
            }
        }
        if (width != 0 && height != 0) {
            CGSize size = core.layer.drawableSize;
            if ((uint32_t)size.width != width || (uint32_t)size.height != height) {
                core.layer.drawableSize = CGSizeMake(width, height);
            }
        }
        if (faults->skip_next_drawable) {
            return CK_PR_FRAME_SKIPPED;
        }
        /* The presenter holds a strong reference to the drawable only inside this step. t3 and t4 bracket the acquisition (the
         * test hold is inside them, so a blocked nextDrawable adds to the acquire interval only). */
        if (result != NULL) {
            result->acquire_begin = ck_now_ns();
        }
        for (int held = 0; faults->hold_acquire != NULL && held < 30000 &&
                           atomic_load_explicit(faults->hold_acquire, memory_order_acquire) != 0; held++) {
            usleep(1000);
        }
        id<CAMetalDrawable> drawable = [core.layer nextDrawable];
        if (result != NULL) {
            result->acquire_end = ck_now_ns();
        }
        if (drawable == nil) {
            return CK_PR_FRAME_SKIPPED;
        }
        ck_obs_add(observer, CK_OBS_DRAWABLES_ACQUIRED, 1);
        ck_obs_add(observer, CK_OBS_DRAWABLE_REFS, 1);
        ck_obs_max(observer, CK_OBS_DRAWABLE_REFS_MAX, atomic_load(&observer->counters[CK_OBS_DRAWABLE_REFS]));
        if (observer == nil) { return CK_PR_FRAME_FAILED; }
        atomic_store(&observer->counters[CK_OBS_LAST_WIDTH], (int64_t)drawable.texture.width);
        atomic_store(&observer->counters[CK_OBS_LAST_HEIGHT], (int64_t)drawable.texture.height);
        /* Its token is independent: the handler captures the token and the observer, nothing else. */
        CKPrTokenState *drawableToken = [CKPrTokenState new];
        drawableToken.label = @"a drawable's presented handler";
        int32_t never_drawable = faults->never_complete_drawable_tokens;
        int32_t never_command = faults->never_complete_command_tokens;
        int32_t delay = faults->handler_delay_ms;
        int32_t completion_delay = faults->completion_delay_ms;
        const _Atomic int32_t *completion_gate = faults->completion_gate;
        [drawable addPresentedHandler:^(id<MTLDrawable> presented) {
            atomic_store(&drawableToken->timingA, ck_seconds_ns(presented.presentedTime));
            ck_set_token(drawableToken, observer, CK_OBS_PRESENTED_RUN, never_drawable, 0, delay, nil, NULL);
        }];
        [core.drawableTokens addObject:drawableToken];
        ck_obs_max(observer, CK_OBS_TOKENS_MAX, (int64_t)core.drawableTokens.count);

        CKPrFrame result = CK_PR_FRAME_DRAWN;
        NSString *reason = nil;
        if (faults->abandon_after_acquire) {
            observer.abandoned = drawable;
            result = CK_PR_FRAME_ABANDONED;
        } else {
            BOOL partial = NO;
            CKMcToken *ledgerToken = NULL;
            id<MTLTexture> capture = nil;
            if (faults->capture) {
                MTLTextureDescriptor *capture_description =
                    [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                                       width:drawable.texture.width
                                                                      height:drawable.texture.height
                                                                   mipmapped:NO];
                capture_description.usage = MTLTextureUsageRenderTarget;
                capture_description.storageMode = MTLStorageModeShared;
                capture = [core.device newTextureWithDescriptor:capture_description];
            }
            id<MTLCommandBuffer> command =
                ck_encode_frame(core.queue, drawable.texture, rgb, faults, core.quadKit, core.quadTexture,
                                quad, packet, core.instanceBuffer, capture, ticket, &ledgerToken, &reason,
                                &partial);
            if (command == nil) {
                observer.abandoned = drawable;
                if (message != NULL && capacity != 0) {
                    ck_message(message, capacity, reason);
                }
                result = CK_PR_FRAME_FAILED;
            } else {
                [command presentDrawable:drawable];
                if (capture != nil) {
                    [command addCompletedHandler:^(id<MTLCommandBuffer> finished) {
                        if (finished.status == MTLCommandBufferStatusCompleted) {
                            @synchronized(observer) {
                                if (observer.captured == nil) {
                                    observer.captured = [NSMutableArray array];
                                }
                                [observer.captured addObject:capture];
                                if (observer.captured.count > CK_CAPTURE_KEEP) {
                                    [observer.captured removeObjectAtIndex:0];
                                    observer.capturedBase += 1;
                                }
                            }
                            ck_obs_add(observer, CK_OBS_CAPTURES, 1);
                        }
                    }];
                }
                CKPrTokenState *commandToken = [CKPrTokenState new];
                commandToken.label = @"a command buffer's completion";
                int32_t forced_error = faults->error_completion;
                [command addCompletedHandler:^(id<MTLCommandBuffer> finished) {
                    /* The GPU time is recorded in the token (handlers only record); the render thread consumes it. */
                    atomic_store(&commandToken->timingA, ck_seconds_ns(finished.GPUStartTime));
                    atomic_store(&commandToken->timingB, ck_seconds_ns(finished.GPUEndTime));
                    /* Test only: the completion waits (bounded) until the test opens its gate. */
                    for (int waited = 0; completion_gate != NULL && waited < 30000 &&
                                         atomic_load_explicit(completion_gate, memory_order_acquire) == 0;
                         waited++) {
                        usleep(1000);
                    }
                    NSString *error = nil;
                    if (forced_error) {
                        error = @"forced command buffer error";
                    } else if (finished.status == MTLCommandBufferStatusError) {
                        error = finished.error.localizedDescription ?: @"the command buffer failed";
                    }
                    if (!never_command) {
                        ck_obs_add(observer, CK_OBS_IN_FLIGHT_NOW, -1);
                    }
                    ck_set_token(commandToken, observer, CK_OBS_COMPLETED_RUN, never_command,
                                 completion_delay, delay, error, ledgerToken);
                }];
                /* Counted before the commit so the completion handler cannot run first: command buffers
                 * committed whose completion has not run, which the one-frame-in-flight rule keeps at one. */
                ck_obs_add(observer, CK_OBS_IN_FLIGHT_NOW, 1);
                ck_obs_max(observer, CK_OBS_IN_FLIGHT_MAX, atomic_load(&observer->counters[CK_OBS_IN_FLIGHT_NOW]));
                /* The clock before the commit: nothing the commit exposes can be presented earlier than this. */
                uint64_t submitted_at = ck_now_ns();
                [command commit];
                for (int held = 0; faults->hold_after_commit != NULL && held < 30000 &&
                                   atomic_load_explicit(faults->hold_after_commit, memory_order_acquire) != 0; held++) {
                    usleep(1000);
                }
                /* t5: the commit returned. Stored in both tokens by this thread, which is also the one that consumes them. */
                uint64_t committed_at = ck_now_ns();
                atomic_store(&commandToken->commitMark, committed_at);
                atomic_store(&drawableToken->commitMark, committed_at);
                if (quad_result != NULL) {
                    quad_result->committed = committed_at;
                    quad_result->submitted = submitted_at;
                }
                /* The predecessor's completion was observed (a frame is committed only when none is
                 * outstanding) and its error was taken in that same observation. */
                core.buffer = command;
                core.bufferToken = commandToken;
                ck_obs_add(observer, CK_OBS_COMMITTED, 1);
                if (quad != NULL && quad->present != 0) {
                    ck_obs_add(observer, CK_OBS_QUAD_DRAWS, 1);
                }
            }
        }
        /* The step is over: the presenter lets go of the drawable (Core Animation and the command buffer
         * own it now, or nobody does and its handler runs when it is released). */
        drawable = nil;
        ck_obs_add(observer, CK_OBS_DRAWABLE_REFS, -1);
        return result;
    }
}

uint32_t ck_pr_take_timing(CKPrCore *handle, CKPrTiming *out, uint32_t capacity, uint64_t *lost) {
    CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
    uint32_t taken = 0;
    while (taken < capacity && core->timingCount != 0) {
        out[taken] = core->timing[core->timingHead];
        core->timingHead = (core->timingHead + 1) % CK_TIMING_RING;
        core->timingCount -= 1;
        taken += 1;
    }
    if (lost != NULL) {
        *lost += core->timingLost;
    }
    core->timingLost = 0;
    return taken;
}

int ck_pr_poll(CKPrCore *handle, char *message, size_t capacity) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        core.entry = CK_ENTRY_POLL;
        (void)ck_command_outstanding(core);
        ck_prune_drawables(core);
        ck_quad_reap(core);
        return ck_take_error(core, message, capacity) ? 1 : 0;
    }
}

int ck_pr_outstanding(CKPrCore *handle) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        core.entry = CK_ENTRY_OUTSTANDING;
        ck_prune_drawables(core);
        ck_quad_reap(core);
        /* The thread sleeps without a timer only when this says 0, so it is decided from one look at the
         * command token (taking its error if it completed), then the latched error, then the drawables. */
        BOOL command = ck_command_outstanding(core);
        BOOL unreported = core.latchedError != nil;
        return command || unreported || ck_drawables_outstanding(core) != 0 || core.retiredTexture != nil
                   ? 1
                   : 0;
    }
}

int ck_pr_tokens_state(CKPrCore *handle, char *outstanding, size_t capacity) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge CKPrCoreState *)handle;
        core.tearingDown = YES;
        core.entry = CK_ENTRY_TOKENS_STATE;
        BOOL command = ck_command_outstanding(core);
        ck_prune_drawables(core); /* the results of the drawables that arrived; the tokens stay for the proof */
        NSUInteger drawables = ck_drawables_outstanding(core);
        BOOL retired = ck_quad_retired_waiting(core);
        if (drawables == 0 && !command && !retired) {
            return 1;
        }
        ck_message(outstanding, capacity,
                   [NSString stringWithFormat:@"%lu drawable presented handler(s) and %@ outstanding%@",
                                              (unsigned long)drawables,
                                              command ? @"1 command buffer completion" : @"no command buffer completion",
                                              retired ? @", and a retired quad texture waiting for its token" : @""]);
        return 0;
    }
}

void ck_pr_release_all(CKPrCore *handle) {
    @autoreleasepool {
        CKPrCoreState *core = (__bridge_transfer CKPrCoreState *)handle;
        CKPrObserverState *observer = core.observer;
        if (observer != nil) {
            atomic_store(&observer->released, true);
            ck_obs_add(observer, CK_OBS_HOLDERS_RELEASED,
                       (core.buffer != nil ? 1 : 0) + (core.queue != nil ? 1 : 0) +
                           (core.device != nil ? 1 : 0) + (core.layer != nil ? 1 : 0));
            if (core.layer != nil) {
                ck_obs_add(observer, CK_OBS_LAYER_RELEASES, 1);
            }
        }
        if (observer != nil) {
            /* The quad's holders: its textures, pipeline and two samplers. A retired texture whose token has
             * not arrived is let go of early: the teardown waits for it, so this stays zero. */
            if (core.retiredTexture != nil && core.retiredToken != nil && !atomic_load(&core.retiredToken->done)) {
                ck_obs_add(observer, CK_OBS_QUAD_RELEASED_EARLY, 1);
            }
            int64_t textures = (core.quadTexture != nil ? 1 : 0) + (core.retiredTexture != nil ? 1 : 0);
            ck_obs_add(observer, CK_OBS_QUAD_TEXTURES_RELEASED, textures);
            ck_obs_add(observer, CK_OBS_QUAD_TEXTURES_ALIVE, -textures);
            ck_obs_add(observer, CK_OBS_QUAD_HOLDERS_RELEASED, textures + (core.quadKit != nil ? 3 : 0));
        }
        core.quadTexture = nil;
        core.retiredTexture = nil;
        core.retiredToken = nil;
        core.quadKit = nil;
        core.instanceBuffer = nil;
        ck_drop_buffer(core);
        core.queue = nil;
        core.device = nil;
        core.layer = nil;
    }
}

int ck_pr_readback(uint32_t width, uint32_t height, const float rgb[3], uint8_t *pixels,
                   size_t length, char *error, size_t capacity) {
    @autoreleasepool {
        if (width == 0 || height == 0 || length != (size_t)width * height * 4) {
            ck_message(error, capacity, @"the readback size does not match the buffer");
            return 1;
        }
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (device == nil) {
            ck_message(error, capacity, @"No Metal device is available");
            return 1;
        }
        id<MTLCommandQueue> queue = [device newCommandQueue];
        MTLTextureDescriptor *description =
            [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                               width:width
                                                              height:height
                                                           mipmapped:NO];
        description.usage = MTLTextureUsageRenderTarget;
        description.storageMode = MTLStorageModeShared;
        id<MTLTexture> texture = [device newTextureWithDescriptor:description];
        if (queue == nil || texture == nil) {
            ck_message(error, capacity, @"the off-screen resources could not be created");
            return 1;
        }
        NSString *reason = nil;
        BOOL partial = NO;
        id<MTLCommandBuffer> command =
            ck_encode_frame(queue, texture, rgb, NULL, nil, nil, NULL, NULL, nil, nil, NULL, NULL, &reason, &partial);
        if (command == nil) {
            ck_message(error, capacity, reason);
            return 1;
        }
        [command commit];
        [command waitUntilCompleted];
        if (command.status != MTLCommandBufferStatusCompleted) {
            ck_message(error, capacity, command.error.localizedDescription ?: @"the off-screen clear failed");
            return 1;
        }
        [texture getBytes:pixels
              bytesPerRow:(NSUInteger)width * 4
               fromRegion:MTLRegionMake2D(0, 0, width, height)
              mipmapLevel:0];
        return 0;
    }
}

int ck_pr_readback_quad(uint32_t width, uint32_t height, const float rgb[3], const uint8_t *library,
                        size_t library_length, const CKPrQuad *quad, const uint8_t *image,
                        size_t image_length, uint8_t *pixels, size_t length, char *error,
                        size_t capacity) {
    @autoreleasepool {
        if (width == 0 || height == 0 || length != (size_t)width * height * 4) {
            ck_message(error, capacity, @"the readback size does not match the buffer");
            return 1;
        }
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (device == nil) {
            ck_message(error, capacity, @"No Metal device is available");
            return 1;
        }
        id<MTLCommandQueue> queue = [device newCommandQueue];
        MTLTextureDescriptor *description =
            [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                               width:width
                                                              height:height
                                                           mipmapped:NO];
        description.usage = MTLTextureUsageRenderTarget;
        description.storageMode = MTLStorageModeShared;
        id<MTLTexture> target = [device newTextureWithDescriptor:description];
        if (queue == nil || target == nil) {
            ck_message(error, capacity, @"the off-screen resources could not be created");
            return 1;
        }
        NSString *reason = nil;
        CKPrQuadKit *kit = nil;
        id<MTLTexture> picture = nil;
        BOOL drawing = quad != NULL && quad->present != 0;
        if (drawing) {
            CKPrUpload upload = {.pixels = image, .length = image_length};
            kit = ck_make_quad_kit(device, library, library_length, 0, &reason);
            if (kit != nil) {
                picture = ck_make_quad_texture(device, quad->width, quad->height, &upload, 0, &reason);
            }
            if (kit == nil || picture == nil) {
                ck_message(error, capacity, reason);
                return 1;
            }
        }
        BOOL partial = NO;
        id<MTLCommandBuffer> command =
            ck_encode_frame(queue, target, rgb, NULL, kit, picture, drawing ? quad : NULL, NULL, nil, nil, NULL,
                            NULL, &reason, &partial);
        if (command == nil) {
            ck_message(error, capacity, reason);
            return 1;
        }
        [command commit];
        [command waitUntilCompleted];
        if (command.status != MTLCommandBufferStatusCompleted) {
            ck_message(error, capacity, command.error.localizedDescription ?: @"the off-screen frame failed");
            return 1;
        }
        [target getBytes:pixels
             bytesPerRow:(NSUInteger)width * 4
              fromRegion:MTLRegionMake2D(0, 0, width, height)
             mipmapLevel:0];
        return 0;
    }
}

int ck_pr_readback_scene(uint32_t width, uint32_t height, const float rgb[3], const uint8_t *library,
                         size_t library_length, const CKPrQuad *quad, const uint8_t *quad_image,
                         size_t quad_image_length, const CKPrReadbackImage *images, size_t image_count,
                         const CKPrReadbackImage *checker, const void *instances, size_t instance_bytes,
                         const CKPrBatch *batches, uint32_t batch_count, int32_t filter, uint32_t table_slots,
                         uint8_t *pixels, size_t length, char *error, size_t capacity) API_AVAILABLE(macos(13.0)) {
    @autoreleasepool {
        if (width == 0 || height == 0 || length != (size_t)width * height * 4 || checker == NULL ||
            (table_slots != 0 && table_slots < image_count + 1) ||
            (batch_count != 0 && (batches == NULL || instances == NULL || instance_bytes == 0))) {
            ck_message(error, capacity, @"the scene readback arguments do not fit");
            return 1;
        }
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (device == nil) {
            ck_message(error, capacity, @"No Metal device is available");
            return 1;
        }
        id<MTLCommandQueue> queue = [device newCommandQueue];
        MTLTextureDescriptor *description =
            [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm
                                                               width:width
                                                              height:height
                                                           mipmapped:NO];
        description.usage = MTLTextureUsageRenderTarget;
        description.storageMode = MTLStorageModeShared;
        id<MTLTexture> target = [device newTextureWithDescriptor:description];
        if (queue == nil || target == nil) {
            ck_message(error, capacity, @"the off-screen resources could not be created");
            return 1;
        }
        NSString *reason = nil;
        CKPrQuadKit *kit = ck_make_quad_kit(device, library, library_length, 0, &reason);
        if (kit == nil) {
            ck_message(error, capacity, reason);
            return 1;
        }
        id<MTLTexture> picture = nil;
        BOOL drawing = quad != NULL && quad->present != 0;
        if (drawing) {
            CKPrUpload upload = {.pixels = quad_image, .length = quad_image_length};
            picture = ck_make_quad_texture(device, quad->width, quad->height, &upload, 0, &reason);
            if (picture == nil) {
                ck_message(error, capacity, reason);
                return 1;
            }
        }
        NSMutableArray<id<MTLTexture>> *textures = [NSMutableArray array];
        for (size_t index = 0; index < image_count; index++) {
            CKPrUpload upload = {.pixels = images[index].pixels, .length = images[index].length};
            id<MTLTexture> texture =
                ck_make_quad_texture(device, images[index].width, images[index].height, &upload, 0, &reason);
            if (texture == nil) {
                ck_message(error, capacity, reason);
                return 1;
            }
            [textures addObject:texture];
        }
        CKPrUpload checker_upload = {.pixels = checker->pixels, .length = checker->length};
        id<MTLTexture> checker_texture =
            ck_make_quad_texture(device, checker->width, checker->height, &checker_upload, 0, &reason);
        if (checker_texture == nil) {
            ck_message(error, capacity, reason);
            return 1;
        }
        CKPrBatch *resolved = batch_count != 0 ? calloc(batch_count, sizeof(CKPrBatch)) : NULL;
        if (batch_count != 0 && resolved == NULL) {
            ck_message(error, capacity, @"the batch list could not be allocated");
            return 1;
        }
        for (uint32_t index = 0; index < batch_count; index++) {
            resolved[index] = batches[index];
            size_t slot = (size_t)(uintptr_t)batches[index].texture;
            if (slot != 0) {
                if (slot > image_count) {
                    free(resolved);
                    ck_message(error, capacity, @"a batch names a texture that was not given");
                    return 1;
                }
                resolved[index].texture = (__bridge void *)textures[slot - 1];
            }
        }
        id<MTLBuffer> buffer = nil;
        if (batch_count != 0) {
            buffer = [device newBufferWithBytes:instances length:instance_bytes options:MTLResourceStorageModeShared];
        }
        CKPrPacket packet = {.batches = resolved,
                             .batch_count = batch_count,
                             .filter = filter,
                             .checker = (__bridge void *)checker_texture};
        /* The bindless readback: the table of `table_slots` resource IDs (slot 0 the checker, 1.. the given images, every other
         * entry the checker, as the ledger sanitises a free slot), and every texture named resident for the encoder. */
        id<MTLBuffer> table = nil;
        const void **resident = NULL;
        if (table_slots != 0) {
            table = [device newBufferWithLength:(NSUInteger)table_slots * sizeof(uint64_t)
                                        options:MTLResourceStorageModeShared];
            resident = calloc(image_count + 1, sizeof(void *));
            if (table == nil || resident == NULL) {
                free(resolved);
                free(resident);
                ck_message(error, capacity, @"the bindless table could not be allocated");
                return 1;
            }
            uint64_t *entries = (uint64_t *)table.contents;
            MTLResourceID checker_id = checker_texture.gpuResourceID;
            memcpy(&entries[0], &checker_id, sizeof(uint64_t));
            resident[0] = (__bridge const void *)checker_texture;
            for (uint32_t slot = 1; slot < table_slots; slot++) {
                MTLResourceID shown = slot <= image_count ? textures[slot - 1].gpuResourceID : checker_id;
                memcpy(&entries[slot], &shown, sizeof(uint64_t));
            }
            for (size_t index = 0; index < image_count; index++) {
                resident[index + 1] = (__bridge const void *)textures[index];
            }
            packet.table = (__bridge void *)table;
            packet.table_slots = table_slots;
            packet.use_resources = resident;
            packet.use_count = (uint32_t)(image_count + 1);
        }
        BOOL partial = NO;
        id<MTLCommandBuffer> command =
            ck_encode_frame(queue, target, rgb, NULL, kit, picture, drawing ? quad : NULL, &packet, buffer, nil,
                            NULL, NULL, &reason, &partial);
        free(resolved);
        if (command == nil) {
            free(resident);
            ck_message(error, capacity, reason);
            return 1;
        }
        [command commit];
        [command waitUntilCompleted];
        free(resident);
        if (command.status != MTLCommandBufferStatusCompleted) {
            ck_message(error, capacity, command.error.localizedDescription ?: @"the off-screen scene failed");
            return 1;
        }
        [target getBytes:pixels
             bytesPerRow:(NSUInteger)width * 4
              fromRegion:MTLRegionMake2D(0, 0, width, height)
             mipmapLevel:0];
        return 0;
    }
}

CKPrObserver *ck_pr_observer_new(void) {
    return (__bridge_retained CKPrObserver *)[CKPrObserverState new];
}

void ck_pr_observer_free(CKPrObserver *observer) {
    if (observer != NULL) {
        __unused CKPrObserverState *owned = (__bridge_transfer CKPrObserverState *)observer;
    }
}

int64_t ck_pr_observer_capture(const CKPrObserver *observer, int64_t which, uint32_t *width, uint32_t *height,
                               uint8_t *pixels, size_t length) {
    @autoreleasepool {
        CKPrObserverState *object = (__bridge CKPrObserverState *)observer;
        id<MTLTexture> texture = nil;
        @synchronized(object) {
            NSInteger count = (NSInteger)object.captured.count;
            if (count != 0) {
                int64_t index = which < 0 ? object.capturedBase + count - 1 : which;
                if (index < object.capturedBase) {
                    return -3;
                }
                if (index < object.capturedBase + count) {
                    texture = object.captured[(NSUInteger)(index - object.capturedBase)];
                }
            }
        }
        if (texture == nil) {
            return -1;
        }
        *width = (uint32_t)texture.width;
        *height = (uint32_t)texture.height;
        if (length != (size_t)texture.width * texture.height * 4) {
            return -2;
        }
        [texture getBytes:pixels
              bytesPerRow:texture.width * 4
               fromRegion:MTLRegionMake2D(0, 0, texture.width, texture.height)
              mipmapLevel:0];
        return atomic_load(&object->counters[CK_OBS_CAPTURES]);
    }
}

int64_t ck_pr_observer_get(const CKPrObserver *observer, int32_t counter) {
    CKPrObserverState *object = (__bridge CKPrObserverState *)observer;
    if (counter < 0 || counter >= CK_OBS_COUNT) {
        return -1;
    }
    return atomic_load(&object->counters[counter]);
}

int32_t ck_pr_observer_abandoned_alive(const CKPrObserver *observer) {
    CKPrObserverState *object = (__bridge CKPrObserverState *)observer;
    return object.abandoned != nil ? 1 : 0;
}

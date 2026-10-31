#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <dispatch/dispatch.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>
#include "textures.h"

static atomic_uint_fast64_t ck_live_contexts;
static atomic_uint_fast64_t ck_live_textures;
static atomic_uint_fast64_t ck_live_batches;
static atomic_uint_fast64_t ck_live_frame_readers;
static atomic_uint_fast64_t ck_live_residency_sets;
static atomic_uint_fast64_t ck_live_residency_members;
static atomic_uint_fast64_t ck_lifecycle_sequence;
static atomic_uint_fast64_t ck_last_residency_remove_order;
static atomic_uint_fast64_t ck_last_texture_release_order;

@interface CKMcContextObject : NSObject
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) id<MTLCommandQueue> queue;
@end
@implementation CKMcContextObject
- (void)dealloc { atomic_fetch_sub_explicit(&ck_live_contexts, 1, memory_order_relaxed); }
@end

@interface CKMcTextureObject : NSObject
@property(nonatomic, strong) CKMcContextObject *context;
@property(nonatomic, strong) id<MTLTexture> texture;
@end
@implementation CKMcTextureObject
- (void)dealloc { atomic_fetch_sub_explicit(&ck_live_textures, 1, memory_order_relaxed); }
@end

struct CKMcContext { __strong CKMcContextObject *object; };
struct CKMcTexture { __strong CKMcTextureObject *object; };

@interface CKMcBatchObject : NSObject
@property(nonatomic, strong) CKMcContextObject *context;
@property(nonatomic, strong) id<MTLBuffer> staging;
@property(nonatomic, strong) id<MTLCommandBuffer> command;
@property(nonatomic, strong) id<MTLBlitCommandEncoder> encoder;
@property(nonatomic, strong) NSMutableArray<CKMcTextureObject *> *textures;
@end
@implementation CKMcBatchObject
- (void)dealloc { atomic_fetch_sub_explicit(&ck_live_batches, 1, memory_order_relaxed); }
@end

@interface CKMcFrameReaderObject : NSObject
@property(nonatomic, strong) CKMcContextObject *context;
@property(nonatomic, strong) CKMcTextureObject *source;
@property(nonatomic, strong) id<MTLTexture> readback;
@property(nonatomic, strong) id<MTLCommandBuffer> command;
@property(nonatomic) BOOL prepared;
@property(nonatomic) BOOL committed;
@end

API_AVAILABLE(macos(15.0))
@interface CKMcResidencyObject : NSObject
@property(nonatomic, strong) CKMcContextObject *context;
@property(nonatomic, strong) id<MTLResidencySet> set;
@property(nonatomic) uint64_t commits;
@end
@implementation CKMcResidencyObject
- (void)dealloc { atomic_fetch_sub_explicit(&ck_live_residency_sets, 1, memory_order_relaxed); }
@end

struct API_AVAILABLE(macos(15.0)) CKMcResidency { __strong CKMcResidencyObject *object; };

API_AVAILABLE(macos(13.0))
@interface CKMcTablesObject : NSObject
@property(nonatomic, strong) CKMcContextObject *context;
@property(nonatomic, strong) id<MTLBuffer> first;
@property(nonatomic, strong) id<MTLBuffer> second;
@end
@implementation CKMcTablesObject
@end

struct API_AVAILABLE(macos(13.0)) CKMcTables { __strong CKMcTablesObject *object; };

#if defined(__ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__) && \
    __ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__ >= 130000
_Static_assert(sizeof(MTLResourceID) == sizeof(uint64_t), "MTLResourceID must be an 8-byte table entry");
_Static_assert(_Alignof(MTLResourceID) == _Alignof(uint64_t), "MTLResourceID alignment must match uint64_t");
#endif

@implementation CKMcFrameReaderObject
- (void)dealloc { atomic_fetch_sub_explicit(&ck_live_frame_readers, 1, memory_order_relaxed); }
@end

struct CKMcBatch { __strong CKMcBatchObject *object; };
struct CKMcFrameReader { __strong CKMcFrameReaderObject *object; };

struct CKMcToken {
    atomic_int done;
    atomic_int status;
    char error[512];
};

static void ck_mc_publish_completion(CKMcToken *token, CKMcCompletionWake callback, void *userdata) {
    // Publish status/error before announcing completion, so a woken owner can always observe it.
    atomic_store_explicit(&token->done, 1, memory_order_release);
    if (callback != NULL) { callback(userdata); }
}

static void ck_mc_delay_completion(CKMcToken *token, CKMcCompletionWake callback, void *userdata,
                                   uint32_t delay_ms) {
    if (delay_ms == 0) {
        ck_mc_publish_completion(token, callback, userdata);
        return;
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)delay_ms * NSEC_PER_MSEC),
                   dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
        ck_mc_publish_completion(token, callback, userdata);
    });
}

static void ck_mc_poll_completion_gate(CKMcToken *token, CKMcCompletionWake callback, void *userdata,
                                       const _Atomic int32_t *gate, uint32_t delay_ms, int waited) {
    if (gate == NULL || atomic_load_explicit(gate, memory_order_acquire) != 0 || waited >= 30000) {
        ck_mc_delay_completion(token, callback, userdata, delay_ms);
        return;
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_MSEC),
                   dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
        ck_mc_poll_completion_gate(token, callback, userdata, gate, delay_ms, waited + 1);
    });
}

static void ck_mc_defer_completion(CKMcToken *token, CKMcCompletionWake callback, void *userdata,
                                   const _Atomic int32_t *gate, uint32_t delay_ms) {
    if (gate == NULL && delay_ms == 0) {
        ck_mc_publish_completion(token, callback, userdata);
        return;
    }
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
        ck_mc_poll_completion_gate(token, callback, userdata, gate, delay_ms, 0);
    });
}

void ck_mc_live_objects(uint64_t *contexts, uint64_t *textures, uint64_t *batches,
                        uint64_t *frame_readers, uint64_t *residency_sets,
                        uint64_t *residency_members, uint64_t *last_residency_remove_order,
                        uint64_t *last_texture_release_order) {
    if (contexts != NULL) { *contexts = atomic_load_explicit(&ck_live_contexts, memory_order_relaxed); }
    if (textures != NULL) { *textures = atomic_load_explicit(&ck_live_textures, memory_order_relaxed); }
    if (batches != NULL) { *batches = atomic_load_explicit(&ck_live_batches, memory_order_relaxed); }
    if (frame_readers != NULL) {
        *frame_readers = atomic_load_explicit(&ck_live_frame_readers, memory_order_relaxed);
    }
    if (residency_sets != NULL) {
        *residency_sets = atomic_load_explicit(&ck_live_residency_sets, memory_order_relaxed);
    }
    if (residency_members != NULL) {
        *residency_members = atomic_load_explicit(&ck_live_residency_members, memory_order_relaxed);
    }
    if (last_residency_remove_order != NULL) {
        *last_residency_remove_order = atomic_load_explicit(&ck_last_residency_remove_order, memory_order_relaxed);
    }
    if (last_texture_release_order != NULL) {
        *last_texture_release_order = atomic_load_explicit(&ck_last_texture_release_order, memory_order_relaxed);
    }
}

static void ck_mc_error(char *buffer, size_t capacity, NSString *message) {
    if (buffer != NULL && capacity != 0) {
        snprintf(buffer, capacity, "%s", message.UTF8String ?: "Unknown Metal error");
    }
}

CKMcContext *ck_mc_context_new(char *error, size_t capacity) {
    @autoreleasepool {
        CKMcContextObject *object = [CKMcContextObject new];
        atomic_fetch_add_explicit(&ck_live_contexts, 1, memory_order_relaxed);
        object.device = MTLCreateSystemDefaultDevice();
        if (object.device == nil) {
            ck_mc_error(error, capacity, @"No Metal device is available");
            return NULL;
        }
        object.queue = [object.device newCommandQueue];
        if (object.queue == nil) {
            ck_mc_error(error, capacity, @"Metal command queue creation failed");
            return NULL;
        }
        CKMcContext *context = calloc(1, sizeof(*context));
        if (context == NULL) {
            ck_mc_error(error, capacity, @"Metal context holder allocation failed");
            return NULL;
        }
        context->object = object;
        return context;
    }
}

void ck_mc_context_free(CKMcContext *context) {
    if (context != NULL) { context->object = nil; free(context); }
}

void *ck_mc_context_device(CKMcContext *context) {
    if (context == NULL || context->object == nil) { return NULL; }
    return (void *)CFBridgingRetain(context->object.device);
}

int64_t ck_mc_context_device_id(CKMcContext *context) {
    if (context == NULL || context->object == nil) { return 0; }
    return (int64_t)(intptr_t)(__bridge void *)context->object.device;
}

int64_t ck_mc_context_queue_id(CKMcContext *context) {
    if (context == NULL || context->object == nil) { return 0; }
    return (int64_t)(intptr_t)(__bridge void *)context->object.queue;
}

void *ck_mc_context_queue(CKMcContext *context) {
    if (context == NULL || context->object == nil) { return NULL; }
    return (void *)CFBridgingRetain(context->object.queue);
}

uint64_t ck_mc_texture_estimate(CKMcContext *context, uint32_t width, uint32_t height) {
    @autoreleasepool {
        if (context == NULL || context->object == nil || width == 0 || height == 0) { return 0; }
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:width height:height mipmapped:NO];
        descriptor.storageMode = MTLStorageModePrivate;
        descriptor.usage = MTLTextureUsageShaderRead;
        MTLSizeAndAlign estimate = [context->object.device heapTextureSizeAndAlignWithDescriptor:descriptor];
        return (uint64_t)estimate.size;
    }
}

CKMcTexture *ck_mc_texture_new(CKMcContext *context, uint32_t width, uint32_t height,
                               uint64_t *allocated, char *error, size_t capacity) {
    @autoreleasepool {
        if (context == NULL || context->object == nil || allocated == NULL || width == 0 || height == 0) {
            ck_mc_error(error, capacity, @"Invalid Metal texture arguments");
            return NULL;
        }
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:width height:height mipmapped:NO];
        descriptor.storageMode = MTLStorageModePrivate;
        descriptor.usage = MTLTextureUsageShaderRead;
        id<MTLTexture> texture = [context->object.device newTextureWithDescriptor:descriptor];
        if (texture == nil) {
            ck_mc_error(error, capacity, @"Private Metal texture allocation failed");
            return NULL;
        }
        CKMcTextureObject *object = [CKMcTextureObject new];
        atomic_fetch_add_explicit(&ck_live_textures, 1, memory_order_relaxed);
        object.context = context->object;
        object.texture = texture;
        CKMcTexture *holder = calloc(1, sizeof(*holder));
        if (holder == NULL) {
            ck_mc_error(error, capacity, @"Metal texture holder allocation failed");
            return NULL;
        }
        holder->object = object;
        *allocated = (uint64_t)texture.allocatedSize;
        return holder;
    }
}

void ck_mc_texture_free(CKMcTexture *texture) {
    if (texture != NULL) {
        uint64_t order = atomic_fetch_add_explicit(&ck_lifecycle_sequence, 1, memory_order_relaxed) + 1;
        atomic_store_explicit(&ck_last_texture_release_order, order, memory_order_relaxed);
        texture->object = nil;
        free(texture);
    }
}

void *ck_mc_texture_metal(const CKMcTexture *texture) {
    return texture == NULL || texture->object == nil ? NULL : (__bridge void *)texture->object.texture;
}

uint64_t ck_mc_texture_size(const CKMcTexture *texture) {
    return texture == NULL || texture->object == nil ? 0 : (uint64_t)texture->object.texture.allocatedSize;
}

int ck_mc_texture_read(CKMcTexture *texture, uint8_t *pixels, size_t length,
                       char *error, size_t capacity) {
    @autoreleasepool {
        if (texture == NULL || texture->object == nil || pixels == NULL ||
            length != (size_t)texture->object.texture.width * texture->object.texture.height * 4) {
            ck_mc_error(error, capacity, @"Invalid Metal texture readback arguments");
            return 1;
        }
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:texture->object.texture.width height:texture->object.texture.height mipmapped:NO];
        descriptor.storageMode = MTLStorageModeShared;
        descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite;
        id<MTLTexture> staging = [texture->object.context.device newTextureWithDescriptor:descriptor];
        id<MTLCommandBuffer> command = [texture->object.context.queue commandBuffer];
        id<MTLBlitCommandEncoder> encoder = [command blitCommandEncoder];
        if (staging == nil || command == nil || encoder == nil) {
            ck_mc_error(error, capacity, @"Metal texture readback resource creation failed");
            return 1;
        }
        [encoder copyFromTexture:texture->object.texture sourceSlice:0 sourceLevel:0
            sourceOrigin:MTLOriginMake(0, 0, 0)
            sourceSize:MTLSizeMake(texture->object.texture.width, texture->object.texture.height, 1)
            toTexture:staging destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
        [encoder endEncoding];
        [command commit];
        [command waitUntilCompleted];
        if (command.status != MTLCommandBufferStatusCompleted) {
            ck_mc_error(error, capacity, command.error.localizedDescription ?: @"Metal texture readback failed");
            return 1;
        }
        [staging getBytes:pixels bytesPerRow:(NSUInteger)texture->object.texture.width * 4
            fromRegion:MTLRegionMake2D(0, 0, texture->object.texture.width, texture->object.texture.height)
            mipmapLevel:0];
        return 0;
    }
}

int ck_mc_measure_shared(CKMcContext *context, uint32_t width, uint32_t height, size_t count,
                         const uint8_t *images, size_t image_length,
                         char *error, size_t capacity) {
    @autoreleasepool {
        size_t one_length = 0;
        if (context == NULL || context->object == nil || width == 0 || height == 0 || count == 0 || images == NULL ||
            (size_t)width > SIZE_MAX / 4 || (size_t)height > SIZE_MAX / ((size_t)width * 4)) {
            ck_mc_error(error, capacity, @"Invalid shared texture measurement arguments");
            return 1;
        }
        one_length = (size_t)width * height * 4;
        if (count > SIZE_MAX / one_length || image_length != count * one_length) {
            ck_mc_error(error, capacity, @"Shared texture measurement byte length mismatch");
            return 1;
        }
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:width height:height mipmapped:NO];
        descriptor.storageMode = MTLStorageModeShared;
        descriptor.usage = MTLTextureUsageShaderRead;
        for (size_t index = 0; index < count; ++index) {
            id<MTLTexture> texture = [context->object.device newTextureWithDescriptor:descriptor];
            if (texture == nil) {
                ck_mc_error(error, capacity, @"Shared Metal texture allocation failed");
                return 1;
            }
            // Shared replacement writes the tightly packed RGBA8 image directly; no command buffer is made.
            [texture replaceRegion:MTLRegionMake2D(0, 0, width, height) mipmapLevel:0
                withBytes:images + index * one_length bytesPerRow:(NSUInteger)width * 4];
        }
        return 0;
    }
}

CKMcBatch *ck_mc_batch_new(CKMcContext *context, uint64_t staging_need,
                           int32_t nil_staging, int32_t nil_command, int32_t nil_encoder,
                           uint64_t *allocated, char *error, size_t capacity) {
    @autoreleasepool {
        if (context == NULL || context->object == nil || allocated == NULL || staging_need == 0 ||
            staging_need > SIZE_MAX) {
            ck_mc_error(error, capacity, @"Invalid Metal batch arguments");
            return NULL;
        }
        CKMcBatchObject *object = [CKMcBatchObject new];
        atomic_fetch_add_explicit(&ck_live_batches, 1, memory_order_relaxed);
        object.context = context->object;
        object.textures = [NSMutableArray array];
        CKMcBatch *batch = calloc(1, sizeof(*batch));
        if (batch == NULL) {
            ck_mc_error(error, capacity, @"Metal batch holder allocation failed");
            return NULL;
        }
        batch->object = object;
        object.staging = nil_staging ? nil : [context->object.device newBufferWithLength:(NSUInteger)staging_need
                                                             options:MTLResourceStorageModeShared];
        if (object.staging == nil) {
            ck_mc_error(error, capacity, @"Metal staging buffer allocation failed");
            return batch;
        }
        *allocated = (uint64_t)object.staging.allocatedSize;
        object.command = nil_command ? nil : [context->object.queue commandBuffer];
        if (object.command == nil) {
            ck_mc_error(error, capacity, @"Metal upload command buffer creation failed");
            return batch;
        }
        object.encoder = nil_encoder ? nil : [object.command blitCommandEncoder];
        if (object.encoder == nil) {
            ck_mc_error(error, capacity, @"Metal upload blit encoder creation failed");
            return batch;
        }
        return batch;
    }
}

CKMcTexture *ck_mc_batch_texture(CKMcBatch *batch, uint32_t width, uint32_t height,
                                 int32_t fail_allocation, uint64_t *allocated,
                                 char *error, size_t capacity) {
    @autoreleasepool {
        if (batch == NULL || batch->object == nil || batch->object.encoder == nil ||
            allocated == NULL || width == 0 || height == 0) {
            ck_mc_error(error, capacity, @"Invalid Metal batch texture arguments");
            return NULL;
        }
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:width height:height mipmapped:NO];
        descriptor.storageMode = MTLStorageModePrivate;
        descriptor.usage = MTLTextureUsageShaderRead;
        id<MTLTexture> texture = fail_allocation ? nil : [batch->object.context.device newTextureWithDescriptor:descriptor];
        if (texture == nil) {
            ck_mc_error(error, capacity, @"Private Metal texture allocation failed");
            return NULL;
        }
        CKMcTextureObject *object = [CKMcTextureObject new];
        atomic_fetch_add_explicit(&ck_live_textures, 1, memory_order_relaxed);
        object.context = batch->object.context;
        object.texture = texture;
        CKMcTexture *holder = calloc(1, sizeof(*holder));
        if (holder == NULL) {
            ck_mc_error(error, capacity, @"Metal texture holder allocation failed");
            return NULL;
        }
        holder->object = object;
        [batch->object.textures addObject:object];
        *allocated = (uint64_t)texture.allocatedSize;
        return holder;
    }
}

void ck_mc_batch_discard_texture(CKMcBatch *batch, CKMcTexture *texture) {
    @autoreleasepool {
        if (batch != NULL && batch->object != nil && texture != NULL && texture->object != nil) {
            [batch->object.textures removeObjectIdenticalTo:texture->object];
        }
    }
}

int ck_mc_batch_copy(CKMcBatch *batch, CKMcTexture *texture, uint32_t width, uint32_t height,
                     uint64_t offset, const uint8_t *pixels, size_t length,
                     char *error, size_t capacity) {
    @autoreleasepool {
        if (batch == NULL || batch->object == nil || batch->object.encoder == nil ||
            texture == NULL || texture->object == nil || pixels == NULL || width == 0 || height == 0 ||
            length != (size_t)width * height * 4 || offset > batch->object.staging.length ||
            length > batch->object.staging.length - (NSUInteger)offset ||
            texture->object.texture.width != width || texture->object.texture.height != height) {
            ck_mc_error(error, capacity, @"Invalid Metal staging copy arguments");
            return 1;
        }
        memcpy((uint8_t *)batch->object.staging.contents + (NSUInteger)offset, pixels, length);
        // The packed buffer offset is 256-byte aligned and RGBA8 rows use width * 4 bytes,
        // satisfying Metal's buffer-to-texture blit offset and row-pitch requirements.
        [batch->object.encoder copyFromBuffer:batch->object.staging sourceOffset:(NSUInteger)offset
            sourceBytesPerRow:(NSUInteger)width * 4 sourceBytesPerImage:length
            sourceSize:MTLSizeMake(width, height, 1) toTexture:texture->object.texture destinationSlice:0
            destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
        return 0;
    }
}

int ck_mc_batch_close(CKMcBatch *batch, char *error, size_t capacity) {
    @autoreleasepool {
        if (batch == NULL || batch->object == nil || batch->object.encoder == nil || batch->object.command == nil) {
            ck_mc_error(error, capacity, @"Metal upload batch is not open");
            return 1;
        }
        [batch->object.encoder endEncoding];
        batch->object.encoder = nil;
        return 0;
    }
}

CKMcToken *ck_mc_batch_commit(CKMcBatch *batch, CKMcCompletionWake callback, void *userdata,
                              int32_t fail_commit, int32_t error_completion,
                              uint32_t completion_delay_ms, const _Atomic int32_t *completion_gate,
                              char *error, size_t capacity) {
    @autoreleasepool {
        if (batch == NULL || batch->object == nil || batch->object.encoder != nil || batch->object.command == nil) {
            ck_mc_error(error, capacity, @"Metal upload batch is not closed");
            return NULL;
        }
        if (fail_commit) {
            ck_mc_error(error, capacity, @"Injected Metal command submission failure");
            return NULL;
        }
        CKMcToken *token = calloc(1, sizeof(*token));
        if (token == NULL) {
            ck_mc_error(error, capacity, @"Metal completion token allocation failed");
            return NULL;
        }
        atomic_init(&token->done, 0);
        atomic_init(&token->status, 0);
        CKMcToken *captured = token;
        [batch->object.command addCompletedHandler:^(id<MTLCommandBuffer> command) {
            if (error_completion || command.status != MTLCommandBufferStatusCompleted) {
                NSString *failure = error_completion ? @"Injected Metal upload completion failure" :
                    command.error.localizedDescription ?: @"Metal upload command failed";
                snprintf(captured->error, sizeof(captured->error), "%s", failure.UTF8String ?: "Metal upload command failed");
                atomic_store_explicit(&captured->status, 1, memory_order_relaxed);
            }
            // Test hooks delay publication asynchronously; Metal's serial completion thread returns at once.
            ck_mc_defer_completion(captured, callback, userdata, completion_gate, completion_delay_ms);
        }];
        [batch->object.command commit];
        return token;
    }
}

CKMcToken *ck_mc_command_token_new(char *error, size_t capacity) {
    CKMcToken *token = calloc(1, sizeof(*token));
    if (token == NULL) {
        ck_mc_error(error, capacity, @"Frame completion token allocation failed");
        return NULL;
    }
    atomic_init(&token->done, 0);
    atomic_init(&token->status, 0);
    return token;
}

void ck_mc_command_token_complete(CKMcToken *token, const char *error_text) {
    if (token == NULL) { return; }
    if (error_text != NULL) {
        snprintf(token->error, sizeof(token->error), "%s", error_text);
        atomic_store_explicit(&token->status, 1, memory_order_relaxed);
    }
    // Publish status/error before announcing completion, so a woken owner can always observe it.
    atomic_store_explicit(&token->done, 1, memory_order_release);
}

int ck_mc_token_done(CKMcToken *token) {
    return token != NULL && atomic_load_explicit(&token->done, memory_order_acquire) != 0;
}

int ck_mc_token_status(CKMcToken *token, char *error, size_t capacity) {
    if (token == NULL || atomic_load_explicit(&token->done, memory_order_acquire) == 0) { return -1; }
    if (atomic_load_explicit(&token->status, memory_order_relaxed) == 0) { return 0; }
    ck_mc_error(error, capacity, [NSString stringWithUTF8String:token->error] ?: @"Metal upload command failed");
    return 1;
}

void ck_mc_token_free(CKMcToken *token) { free(token); }

uint64_t ck_mc_batch_staging_size(const CKMcBatch *batch) {
    return batch == NULL || batch->object == nil ? 0 : (uint64_t)batch->object.staging.allocatedSize;
}

void ck_mc_batch_free(CKMcBatch *batch) {
    if (batch != NULL) {
        if (batch->object != nil && batch->object.encoder != nil) {
            [batch->object.encoder endEncoding];
            batch->object.encoder = nil;
        }
        batch->object = nil;
        free(batch);
    }
}

CKMcFrameReader *ck_mc_frame_reader_new(CKMcContext *context, CKMcTexture *source,
                                        char *error, size_t capacity) {
    @autoreleasepool {
        if (context == NULL || context->object == nil || source == NULL || source->object == nil) {
            ck_mc_error(error, capacity, @"Invalid frame reader arguments");
            return NULL;
        }
        CKMcFrameReaderObject *object = [CKMcFrameReaderObject new];
        atomic_fetch_add_explicit(&ck_live_frame_readers, 1, memory_order_relaxed);
        object.context = context->object;
        object.source = source->object;
        MTLTextureDescriptor *descriptor = [MTLTextureDescriptor
            texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
            width:source->object.texture.width height:source->object.texture.height mipmapped:NO];
        descriptor.storageMode = MTLStorageModeShared;
        descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite;
        object.readback = [context->object.device newTextureWithDescriptor:descriptor];
        object.command = [context->object.queue commandBuffer];
        id<MTLBlitCommandEncoder> encoder = [object.command blitCommandEncoder];
        if (object.readback == nil || object.command == nil || encoder == nil) {
            ck_mc_error(error, capacity, @"Frame reader command resources could not be created");
            return NULL;
        }
        [encoder copyFromTexture:object.source.texture sourceSlice:0 sourceLevel:0
            sourceOrigin:MTLOriginMake(0, 0, 0)
            sourceSize:MTLSizeMake(object.source.texture.width, object.source.texture.height, 1)
            toTexture:object.readback destinationSlice:0 destinationLevel:0
            destinationOrigin:MTLOriginMake(0, 0, 0)];
        [encoder endEncoding];
        CKMcFrameReader *reader = calloc(1, sizeof(*reader));
        if (reader == NULL) {
            ck_mc_error(error, capacity, @"Frame reader holder allocation failed");
            return NULL;
        }
        reader->object = object;
        return reader;
    }
}

CKMcToken *ck_mc_frame_reader_prepare(CKMcFrameReader *reader, uint32_t completion_delay_ms,
                                      int32_t error_completion, char *error, size_t capacity) {
    @autoreleasepool {
        if (reader == NULL || reader->object == nil || reader->object.command == nil ||
            reader->object.prepared || reader->object.committed) {
            ck_mc_error(error, capacity, @"Frame reader is not available for submission");
            return NULL;
        }
        CKMcToken *token = calloc(1, sizeof(*token));
        if (token == NULL) {
            ck_mc_error(error, capacity, @"Frame completion token allocation failed");
            return NULL;
        }
        atomic_init(&token->done, 0);
        atomic_init(&token->status, 0);
        CKMcToken *captured = token;
        [reader->object.command addCompletedHandler:^(id<MTLCommandBuffer> command) {
            if (error_completion || command.status != MTLCommandBufferStatusCompleted) {
                NSString *failure = error_completion ? @"Injected frame completion failure" :
                    command.error.localizedDescription ?: @"Frame command failed";
                snprintf(captured->error, sizeof(captured->error), "%s",
                         failure.UTF8String ?: "Frame command failed");
                atomic_store_explicit(&captured->status, 1, memory_order_relaxed);
            }
            ck_mc_defer_completion(captured, NULL, NULL, NULL, completion_delay_ms);
        }];
        reader->object.prepared = YES;
        return token;
    }
}

int ck_mc_frame_reader_commit(CKMcFrameReader *reader, char *error, size_t capacity) {
    @autoreleasepool {
        if (reader == NULL || reader->object == nil || !reader->object.prepared ||
            reader->object.committed || reader->object.command == nil) {
            ck_mc_error(error, capacity, @"Frame reader was not prepared or was already committed");
            return 1;
        }
        [reader->object.command commit];
        reader->object.committed = YES;
        return 0;
    }
}

int ck_mc_frame_reader_read(CKMcFrameReader *reader, uint8_t *pixels, size_t length,
                            char *error, size_t capacity) {
    @autoreleasepool {
        if (reader == NULL || reader->object == nil || !reader->object.committed ||
            pixels == NULL || length != (size_t)reader->object.readback.width *
            reader->object.readback.height * 4) {
            ck_mc_error(error, capacity, @"Invalid frame reader readback arguments");
            return 1;
        }
        [reader->object.readback getBytes:pixels bytesPerRow:(NSUInteger)reader->object.readback.width * 4
            fromRegion:MTLRegionMake2D(0, 0, reader->object.readback.width, reader->object.readback.height)
            mipmapLevel:0];
        return 0;
    }
}

void ck_mc_frame_reader_free(CKMcFrameReader *reader) {
    if (reader != NULL) { reader->object = nil; free(reader); }
}

CKMcResidency *ck_mc_residency_new(CKMcContext *context, uint32_t slots, int32_t force_tier1,
                                   int32_t fail_creation, char *error, size_t capacity) {
    @autoreleasepool {
        if (context == NULL || context->object == nil) {
            ck_mc_error(error, capacity, @"bindless capability gate: Metal context is unavailable");
            return NULL;
        }
        id<MTLDevice> device = context->object.device;
        if (force_tier1 || device.argumentBuffersSupport != MTLArgumentBuffersTier2) {
            ck_mc_error(error, capacity, [NSString stringWithFormat:
                @"bindless capability gate: argumentBuffersSupport requested Tier 2, available Tier %lu",
                (unsigned long)device.argumentBuffersSupport]);
            return NULL;
        }
        if (@available(macOS 10.15, *)) {
            if (![device supportsFamily:MTLGPUFamilyApple6]) {
                ck_mc_error(error, capacity, @"bindless capability gate: Apple GPU family requested Apple6 or later, highest available Apple family is below Apple6");
                return NULL;
            }
        } else {
            ck_mc_error(error, capacity, @"bindless capability gate: Apple GPU family requested Apple6 or later, unavailable before macOS 10.15");
            return NULL;
        }
        if (@available(macOS 15.0, *)) {
            if (fail_creation) {
                ck_mc_error(error, capacity, [NSString stringWithFormat:
                    @"bindless capability gate: MTLResidencySet requested capacity %u, available 0 (injected creation failure)", slots]);
                return NULL;
            }
            MTLResidencySetDescriptor *descriptor = [MTLResidencySetDescriptor new];
            descriptor.initialCapacity = slots;
            NSError *native_error = nil;
            id<MTLResidencySet> residency = [device newResidencySetWithDescriptor:descriptor error:&native_error];
            if (residency == nil) {
                ck_mc_error(error, capacity, [NSString stringWithFormat:
                    @"bindless capability gate: MTLResidencySet requested capacity %u, available 0 (%@)", slots, native_error.localizedDescription ?: @"creation refused"]);
                return NULL;
            }
            [context->object.queue addResidencySet:residency];
            CKMcResidencyObject *object = [CKMcResidencyObject new];
            object.context = context->object;
            object.set = residency;
            atomic_fetch_add_explicit(&ck_live_residency_sets, 1, memory_order_relaxed);
            CKMcResidency *holder = calloc(1, sizeof(*holder));
            if (holder == NULL) {
                [context->object.queue removeResidencySet:residency];
                ck_mc_error(error, capacity, @"bindless capability gate: residency holder allocation failed");
                return NULL;
            }
            holder->object = object;
            return holder;
        }
        ck_mc_error(error, capacity, [NSString stringWithFormat:
            @"bindless capability gate: MTLResidencySet requested macOS 15.0, available deployment runtime below 15.0 (slots %u)", slots]);
        return NULL;
    }
}

void ck_mc_residency_add(CKMcResidency *set, void *texture) {
    if (set != NULL && set->object != nil && texture != NULL) {
        [set->object.set addAllocation:(__bridge id<MTLAllocation>)texture];
        atomic_fetch_add_explicit(&ck_live_residency_members, 1, memory_order_relaxed);
    }
}

void ck_mc_residency_remove(CKMcResidency *set, void *texture) {
    if (set != NULL && set->object != nil && texture != NULL) {
        [set->object.set removeAllocation:(__bridge id<MTLAllocation>)texture];
        atomic_fetch_sub_explicit(&ck_live_residency_members, 1, memory_order_relaxed);
        uint64_t order = atomic_fetch_add_explicit(&ck_lifecycle_sequence, 1, memory_order_relaxed) + 1;
        atomic_store_explicit(&ck_last_residency_remove_order, order, memory_order_relaxed);
    }
}

void ck_mc_residency_commit(CKMcResidency *set) {
    if (set != NULL && set->object != nil) {
        [set->object.set commit];
        set->object.commits += 1;
    }
}

uint64_t ck_mc_residency_commit_count(CKMcResidency *set) {
    return set == NULL || set->object == nil ? 0 : set->object.commits;
}

uint64_t ck_mc_residency_size(CKMcResidency *set) {
    return set == NULL || set->object == nil ? 0 : set->object.set.allocatedSize;
}

void ck_mc_residency_free(CKMcResidency *set) {
    if (set == NULL) { return; }
    if (set->object != nil) {
        [set->object.context.queue removeResidencySet:set->object.set];
        set->object = nil;
    }
    free(set);
}

CKMcTables *ck_mc_tables_new(CKMcContext *context, uint32_t slots, uint64_t *allocated,
                             char *error, size_t capacity) {
    @autoreleasepool {
        if (context == NULL || context->object == nil || slots == 0) {
            ck_mc_error(error, capacity, @"bindless table creation needs a live context and nonzero slot count");
            return NULL;
        }
        uint64_t length = (uint64_t)slots * sizeof(MTLResourceID);
        if (length > NSUIntegerMax) {
            ck_mc_error(error, capacity, @"bindless table byte length exceeds NSUInteger");
            return NULL;
        }
        CKMcTablesObject *object = [CKMcTablesObject new];
        object.context = context->object;
        object.first = [context->object.device newBufferWithLength:(NSUInteger)length
                                                           options:MTLResourceStorageModeShared];
        object.second = [context->object.device newBufferWithLength:(NSUInteger)length
                                                            options:MTLResourceStorageModeShared];
        if (object.first == nil || object.second == nil || object.first.contents == NULL ||
            object.second.contents == NULL) {
            ck_mc_error(error, capacity, [NSString stringWithFormat:
                @"bindless table creation requested %llu bytes per table, available 0",
                (unsigned long long)length]);
            return NULL;
        }
        memset(object.first.contents, 0, (size_t)length);
        memset(object.second.contents, 0, (size_t)length);
        if (allocated != NULL) {
            *allocated = (uint64_t)object.first.allocatedSize + (uint64_t)object.second.allocatedSize;
        }
        CKMcTables *tables = calloc(1, sizeof(*tables));
        if (tables == NULL) {
            ck_mc_error(error, capacity, @"bindless table holder allocation failed");
            return NULL;
        }
        tables->object = object;
        return tables;
    }
}

uint64_t ck_mc_texture_resource_id(CKMcTexture *texture) {
    if (texture == NULL || texture->object == nil || texture->object.texture == nil) { return 0; }
    MTLResourceID identifier = texture->object.texture.gpuResourceID;
    uint64_t value = 0;
    memcpy(&value, &identifier, sizeof(value));
    return value;
}

void *ck_mc_tables_buffer(CKMcTables *tables, uint32_t version) {
    if (tables == NULL || tables->object == nil) { return NULL; }
    id<MTLBuffer> buffer = version == 0 ? tables->object.first : tables->object.second;
    return (__bridge void *)buffer;
}

int ck_mc_tables_write(CKMcTables *tables, uint32_t version, uint32_t slot, uint64_t resource_id) {
    if (tables == NULL || tables->object == nil || version > 1) { return 1; }
    id<MTLBuffer> buffer = version == 0 ? tables->object.first : tables->object.second;
    if (slot >= buffer.length / sizeof(MTLResourceID) || buffer.contents == NULL) { return 1; }
    MTLResourceID identifier;
    memcpy(&identifier, &resource_id, sizeof(identifier));
    memcpy((uint8_t *)buffer.contents + (size_t)slot * sizeof(identifier), &identifier, sizeof(identifier));
    return 0;
}

uint64_t ck_mc_tables_size(CKMcTables *tables) {
    if (tables == NULL || tables->object == nil) { return 0; }
    return (uint64_t)tables->object.first.allocatedSize + (uint64_t)tables->object.second.allocatedSize;
}

void ck_mc_tables_free(CKMcTables *tables) {
    if (tables != NULL) { tables->object = nil; free(tables); }
}

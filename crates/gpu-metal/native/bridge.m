#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdio.h>
#include <string.h>
#include "bridge.h"

@interface CKContext : NSObject
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) id<MTLCommandQueue> queue;
@property(nonatomic, strong) id<MTLComputePipelineState> pipeline;
@end
@implementation CKContext
@end

static void ck_error(char *buffer, size_t capacity, NSString *message) {
    if (buffer != NULL && capacity != 0) {
        snprintf(buffer, capacity, "%s", message.UTF8String ?: "Unknown Metal error");
    }
}

CKMetalHandle *ck_metal_create(const uint8_t *bytes, size_t length,
                              char *error, size_t capacity) {
    @autoreleasepool {
        CKContext *context = [CKContext new];
        context.device = MTLCreateSystemDefaultDevice();
        if (context.device == nil) {
            ck_error(error, capacity, @"No Metal device is available");
            return NULL;
        }
        NSError *failure = nil;
        dispatch_data_t data = dispatch_data_create(bytes, length,
            dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), DISPATCH_DATA_DESTRUCTOR_DEFAULT);
        id<MTLLibrary> library = [context.device newLibraryWithData:data error:&failure];
        if (library == nil) {
            ck_error(error, capacity, failure.localizedDescription);
            return NULL;
        }
        id<MTLFunction> function = [library newFunctionWithName:@"ck_probe"];
        if (function == nil) {
            ck_error(error, capacity, @"Compiled shader library has no ck_probe function");
            return NULL;
        }
        context.pipeline = [context.device newComputePipelineStateWithFunction:function error:&failure];
        context.queue = [context.device newCommandQueue];
        if (context.pipeline == nil || context.queue == nil) {
            ck_error(error, capacity, failure.localizedDescription ?: @"Metal queue creation failed");
            return NULL;
        }
        return (__bridge_retained CKMetalHandle *)context;
    }
}

int ck_metal_probe(CKMetalHandle *handle, uint32_t *output, size_t count,
                   char *error, size_t capacity) {
    @autoreleasepool {
        if (handle == NULL || output == NULL || count != 64) {
            ck_error(error, capacity, @"Invalid probe arguments");
            return 1;
        }
        CKContext *context = (__bridge CKContext *)handle;
        id<MTLBuffer> buffer = [context.device newBufferWithLength:64 * sizeof(uint32_t)
                                                        options:MTLResourceStorageModeShared];
        id<MTLCommandBuffer> command = [context.queue commandBuffer];
        id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
        if (buffer == nil || command == nil || encoder == nil) {
            ck_error(error, capacity, @"Could not allocate Metal probe resources");
            return 1;
        }
        [encoder setComputePipelineState:context.pipeline];
        [encoder setBuffer:buffer offset:0 atIndex:0];
        NSUInteger width = MIN((NSUInteger)64, context.pipeline.maxTotalThreadsPerThreadgroup);
        [encoder dispatchThreads:MTLSizeMake(64, 1, 1)
           threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
        [encoder endEncoding];
        [command commit];
        [command waitUntilCompleted];
        if (command.status != MTLCommandBufferStatusCompleted) {
            ck_error(error, capacity, command.error.localizedDescription ?: @"Metal command failed");
            return 1;
        }
        memcpy(output, buffer.contents, 64 * sizeof(uint32_t));
        return 0;
    }
}

void ck_metal_destroy(CKMetalHandle *handle) {
    @autoreleasepool {
        if (handle != NULL) {
            __unused CKContext *context = (__bridge_transfer CKContext *)handle;
        }
    }
}

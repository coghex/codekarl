#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include "../../native/textures.h"
#include "bindless_probe.h"

@interface CKTestBindlessProbeGate : NSObject {
@public
    _Atomic int gate;
    _Atomic int done;
    _Atomic int failed;
}
@property(nonatomic) BOOL holdCompletion;
@end
@implementation CKTestBindlessProbeGate
@end

@interface CKTestBindlessProbeObject : NSObject
@property(nonatomic, strong) id<MTLBuffer> table;
@property(nonatomic, strong) id<MTLBuffer> output;
@property(nonatomic, strong) id<MTLCommandBuffer> command;
@property(nonatomic, strong) id<MTLComputePipelineState> pipeline;
@property(nonatomic, strong) CKTestBindlessProbeGate *gate;
@end
@implementation CKTestBindlessProbeObject
@end

struct CKTestBindlessProbe {
    void *object;
};

static void ck_test_probe_error(char *buffer, size_t capacity, NSString *message) {
    if (buffer != NULL && capacity != 0) {
        snprintf(buffer, capacity, "%s", message.UTF8String ?: "Unknown bindless probe error");
    }
}

CKTestBindlessProbe *ck_test_bindless_probe_new(void *raw_context, void *raw_table,
                                                const uint8_t *bytes, size_t length,
                                                uint32_t slot, int32_t hold_completion,
                                                char *error, size_t capacity) {
    @autoreleasepool {
        CKMcContext *context = raw_context;
        id<MTLBuffer> table = (__bridge id<MTLBuffer>)raw_table;
        if (context == NULL || table == nil || bytes == NULL || length == 0 || slot > 0 || table.length < 8 * sizeof(uint64_t)) {
            ck_test_probe_error(error, capacity, @"invalid bindless probe arguments");
            return NULL;
        }
        id<MTLDevice> device = (__bridge_transfer id<MTLDevice>)ck_mc_context_device(context);
        id<MTLCommandQueue> queue = (__bridge_transfer id<MTLCommandQueue>)ck_mc_context_queue(context);
        if (device == nil || queue == nil) {
            ck_test_probe_error(error, capacity, @"Metal context has no device or queue");
            return NULL;
        }
        NSError *failure = nil;
        dispatch_data_t data = dispatch_data_create(bytes, length,
            dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{});
        id<MTLLibrary> library = [device newLibraryWithData:data error:&failure];
        id<MTLFunction> function = [library newFunctionWithName:@"ck_bindless_probe"];
        id<MTLComputePipelineState> pipeline = function == nil ? nil :
            [device newComputePipelineStateWithFunction:function error:&failure];
        id<MTLCommandBuffer> command = [queue commandBuffer];
        id<MTLBuffer> output = [device newBufferWithLength:8 * sizeof(uint32_t)
                                                                  options:MTLResourceStorageModeShared];
        id<MTLComputeCommandEncoder> encoder = command == nil ? nil : [command computeCommandEncoder];
        if (library == nil || pipeline == nil || command == nil || output == nil || encoder == nil || table.length < 8 * sizeof(uint64_t)) {
            ck_test_probe_error(error, capacity, failure.localizedDescription ?: @"could not prepare bindless probe resources");
            return NULL;
        }
        [encoder setComputePipelineState:pipeline];
        [encoder setBuffer:table offset:0 atIndex:0];
        [encoder setBuffer:output offset:0 atIndex:1];
        [encoder setBytes:&slot length:sizeof(slot) atIndex:2];
        [encoder dispatchThreads:MTLSizeMake(8, 1, 1) threadsPerThreadgroup:MTLSizeMake(8, 1, 1)];
        [encoder endEncoding];
        CKTestBindlessProbeObject *object = [CKTestBindlessProbeObject new];
        object.table = table;
        object.output = output;
        object.command = command;
        object.pipeline = pipeline;
        object.gate = [CKTestBindlessProbeGate new];
        object.gate.holdCompletion = hold_completion != 0;
        atomic_init(&object.gate->gate, hold_completion == 0);
        atomic_init(&object.gate->done, 0);
        atomic_init(&object.gate->failed, 0);
        CKTestBindlessProbe *probe = calloc(1, sizeof(*probe));
        if (probe == NULL) {
            ck_test_probe_error(error, capacity, @"bindless probe holder allocation failed");
            return NULL;
        }
        probe->object = (__bridge_retained void *)object;
        return probe;
    }
}

int ck_test_bindless_probe_prepare(CKTestBindlessProbe *probe, char *error, size_t capacity) {
    @autoreleasepool {
        CKTestBindlessProbeObject *object = probe == NULL ? nil : (__bridge CKTestBindlessProbeObject *)probe->object;
        if (object == nil || object.command == nil) {
            ck_test_probe_error(error, capacity, @"bindless probe is unavailable");
            return 1;
        }
        CKTestBindlessProbeGate *gate = object.gate;
        BOOL hold = gate.holdCompletion;
        [object.command addCompletedHandler:^(id<MTLCommandBuffer> command) {
            const BOOL succeeded = command.status == MTLCommandBufferStatusCompleted;
            dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
                if (hold) {
                    for (unsigned attempt = 0; attempt < 30000 && atomic_load_explicit(&gate->gate, memory_order_acquire) == 0; ++attempt) {
                        usleep(1000);
                    }
                }
                if (!succeeded) { atomic_store_explicit(&gate->failed, 1, memory_order_release); }
                atomic_store_explicit(&gate->done, 1, memory_order_release);
            });
        }];
        return 0;
    }
}

int ck_test_bindless_probe_commit(CKTestBindlessProbe *probe, char *error, size_t capacity) {
    CKTestBindlessProbeObject *object = probe == NULL ? nil : (__bridge CKTestBindlessProbeObject *)probe->object;
    if (object == nil || object.command == nil) {
        ck_test_probe_error(error, capacity, @"bindless probe is unavailable");
        return 1;
    }
    [object.command commit];
    return 0;
}

int ck_test_bindless_probe_gpu_done(CKTestBindlessProbe *probe) {
    CKTestBindlessProbeObject *object = probe == NULL ? nil : (__bridge CKTestBindlessProbeObject *)probe->object;
    return object != nil && object.command.status == MTLCommandBufferStatusCompleted;
}

int ck_test_bindless_probe_read(CKTestBindlessProbe *probe, uint8_t *output,
                                size_t length, char *error, size_t capacity) {
    CKTestBindlessProbeObject *object = probe == NULL ? nil : (__bridge CKTestBindlessProbeObject *)probe->object;
    if (object == nil || output == NULL || length != 8 * sizeof(uint32_t) ||
        object.command.status != MTLCommandBufferStatusCompleted ||
        atomic_load_explicit(&object.gate->done, memory_order_acquire) == 0 ||
        atomic_load_explicit(&object.gate->failed, memory_order_acquire) != 0) {
        ck_test_probe_error(error, capacity, @"bindless probe readback requested before successful completion");
        return 1;
    }
    memcpy(output, object.output.contents, length);
    return 0;
}

void ck_test_bindless_probe_open_gate(CKTestBindlessProbe *probe) {
    if (probe != NULL && probe->object != NULL) {
        CKTestBindlessProbeObject *object = (__bridge CKTestBindlessProbeObject *)probe->object;
        atomic_store_explicit(&object.gate->gate, 1, memory_order_release);
    }
}

int ck_test_bindless_table_entry(void *raw_table, uint32_t slots, uint32_t slot,
                                 uint64_t *resource_id) {
    id<MTLBuffer> table = (__bridge id<MTLBuffer>)raw_table;
    if (table == nil || resource_id == NULL || slot >= slots ||
        table.length < (NSUInteger)slots * sizeof(uint64_t) || table.contents == NULL) {
        return 1;
    }
    memcpy(resource_id, (const uint8_t *)table.contents + (size_t)slot * sizeof(*resource_id), sizeof(*resource_id));
    return 0;
}

uint64_t ck_test_bindless_table_length(void *raw_table) {
    id<MTLBuffer> table = (__bridge id<MTLBuffer>)raw_table;
    return table == nil ? 0 : (uint64_t)table.length;
}

void ck_test_bindless_probe_free(CKTestBindlessProbe *probe) {
    if (probe != NULL) {
        CKTestBindlessProbeObject *object = (__bridge CKTestBindlessProbeObject *)probe->object;
        if (object != nil) {
            atomic_store_explicit(&object.gate->gate, 1, memory_order_release);
        }
        (void)CFBridgingRelease(probe->object);
        free(probe);
    }
}

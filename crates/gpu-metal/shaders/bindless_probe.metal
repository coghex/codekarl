#include <metal_stdlib>

using namespace metal;

struct CKBindlessTable {
    array<texture2d<float, access::read>, 8> textures [[id(0)]];
};

// The table is the Tier 2 argument buffer: each 8-byte MTLResourceID is one texture entry.
// Keep this probe deliberately small; the integration fixture allocates exactly eight slots.
kernel void ck_bindless_probe(
    constant CKBindlessTable &table [[buffer(0)]],
    device uchar4 *output [[buffer(1)]],
    constant uint &base_slot [[buffer(2)]],
    uint index [[thread_position_in_grid]]) {
    const float4 pixel = table.textures[base_slot + index].read(uint2(0, 0));
    output[index] = uchar4(round(saturate(pixel) * 255.0f));
}

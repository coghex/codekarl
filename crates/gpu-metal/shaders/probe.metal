#include <metal_stdlib>
using namespace metal;

kernel void ck_probe(device uint *output [[buffer(0)]], uint index [[thread_position_in_grid]]) {
    if (index < 64) {
        output[index] = index * 3 + 7;
    }
}

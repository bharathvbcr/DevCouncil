#include <metal_stdlib>
using namespace metal;

float scale(float v) {
    return v * 2.0f;
}

kernel void add(device float *out [[buffer(0)]], uint id [[thread_position_in_grid]]) {
    out[id] = scale(out[id]);
}

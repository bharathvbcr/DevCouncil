#include <metal_stdlib>
using namespace metal;
kernel void add(device float *values [[buffer(0)]], uint id [[thread_position_in_grid]]) { values[id] += 1.0; }

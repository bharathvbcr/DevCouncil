#include <metal_stdlib>
using namespace metal;

inline float twice(float x) {
    return x * 2.0f;
}

kernel void plain_scale(
    device float *out [[buffer(0)]],
    uint gid [[thread_position_in_grid]])
{
    out[gid] = twice(out[gid]);
}

#define ROWS_KERNEL(NAME, D, R)                                  \
kernel void NAME(                                                \
    device float *out [[buffer(0)]],                             \
    uint gid [[thread_position_in_grid]])                        \
{                                                                \
    out[gid] = twice(out[gid]) * (D) + (R);                      \
}

#define GATE_IMPL(T, SUFFIX, NAME)                               \
kernel void NAME(device T *x [[buffer(0)]],                      \
                 uint gid [[thread_position_in_grid]])           \
{                                                                \
    x[gid] = x[gid] + T(1);                                      \
}

#define GATE_KERNEL(NAME, T) GATE_IMPL(T, _g, NAME)

ROWS_KERNEL(rows_h256_r16, 256, 16)
ROWS_KERNEL(rows_h512_r32, 512, 32)
GATE_IMPL(float, _f, gate_direct_f32)
GATE_KERNEL(gate_f32, float)

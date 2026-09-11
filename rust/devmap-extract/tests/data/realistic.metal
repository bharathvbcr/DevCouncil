#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

constexpr constant float kEpsilon = 1e-6f;
constant float3 kLuma = float3(0.2126f, 0.7152f, 0.0722f);

struct VertexIn {
    float3 position [[attribute(0)]];
    float3 normal   [[attribute(1)]];
    float2 uv       [[attribute(2)]];
};

struct VertexOut {
    float4 clip [[position]];
    float2 uv;
    float  fog [[flat]];
};

struct Uniforms {
    float4x4 modelViewProjection;
    float3   cameraPosition;
    float    time;
};

namespace shading {

inline float luminance(float3 color) {
    return dot(color, kLuma);
}

float3 tonemap(float3 color, float exposure) {
    float3 mapped = color * exposure;
    return mapped / (mapped + float3(1.0f));
}

}  // namespace shading

template <typename T>
inline T lerpValue(T a, T b, float t) {
    return a + (b - a) * t;
}

inline float safeDivide(float numerator, float denominator) {
    return numerator / max(denominator, kEpsilon);
}

kernel void residual_add(device float *output [[buffer(0)]],
                         const device float *residual [[buffer(1)]],
                         constant Uniforms &uniforms [[buffer(2)]],
                         uint gid [[thread_position_in_grid]]) {
    float scaled = safeDivide(residual[gid], uniforms.time);
    output[gid] = lerpValue(output[gid], scaled, 0.5f);
}

kernel void reduce_max(const device float *input [[buffer(0)]],
                       device float *output [[buffer(1)]],
                       threadgroup float *scratch [[threadgroup(0)]],
                       uint tid [[thread_position_in_threadgroup]],
                       uint gid [[thread_position_in_grid]],
                       uint groupId [[threadgroup_position_in_grid]]) {
    scratch[tid] = input[gid];
    threadgroup_barrier(mem_flags::mem_threadgroup);
    for (uint stride = 1; stride < 32; stride <<= 1) {
        if (tid % (2 * stride) == 0) {
            scratch[tid] = max(scratch[tid], scratch[tid + stride]);
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }
    if (tid == 0) {
        output[groupId] = scratch[0];
    }
}

kernel void blur_texture(texture2d<float, access::read> src [[texture(0)]],
                         texture2d<float, access::write> dst [[texture(1)]],
                         uint2 gid [[thread_position_in_grid]]) {
    float4 accumulated = float4(0.0f);
    for (int dy = -1; dy <= 1; ++dy) {
        for (int dx = -1; dx <= 1; ++dx) {
            accumulated += src.read(uint2(gid.x + dx, gid.y + dy));
        }
    }
    dst.write(accumulated / 9.0f, gid);
}

vertex VertexOut scene_vertex(VertexIn in [[stage_in]],
                              constant Uniforms &uniforms [[buffer(1)]],
                              uint vertexId [[vertex_id]]) {
    VertexOut out;
    out.clip = uniforms.modelViewProjection * float4(in.position, 1.0f);
    out.uv = in.uv;
    out.fog = safeDivide(float(vertexId), uniforms.time);
    return out;
}

fragment float4 scene_fragment(VertexOut in [[stage_in]],
                               texture2d<float> albedo [[texture(0)]],
                               sampler albedoSampler [[sampler(0)]],
                               constant Uniforms &uniforms [[buffer(0)]]) {
    float4 sampled = albedo.sample(albedoSampler, in.uv);
    float3 mapped = shading::tonemap(sampled.rgb, uniforms.time);
    float key = shading::luminance(mapped);
    return float4(mapped * lerpValue(1.0f, key, in.fog), sampled.a);
}

[[kernel]] void tagged_entry(device float *values [[buffer(0)]],
                             uint id [[thread_position_in_grid]]) {
    values[id] = shading::luminance(float3(values[id]));
}

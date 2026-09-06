#include <cuda_runtime.h>

__device__ float scale(float v) {
    return v * 2.0f;
}

__global__ void add(float *out) {
    out[threadIdx.x] = scale(out[threadIdx.x]);
}

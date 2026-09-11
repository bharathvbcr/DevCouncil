#include <cuda_runtime.h>

class BaseKernel {
public:
    virtual float scale(float v);
};

class Widget : public BaseKernel {
public:
    float scale(float v) { return v * 2.0f; }
};

__device__ float scale(float v) {
    return v * 2.0f;
}

__global__ void add(float *out) {
    out[threadIdx.x] = scale(out[threadIdx.x]);
}

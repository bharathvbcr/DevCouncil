#include <cuda_runtime.h>
__global__ void add(float *values) { values[threadIdx.x] += 1.0f; }

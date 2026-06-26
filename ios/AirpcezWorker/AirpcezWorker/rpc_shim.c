#include "rpc_shim.h"
#include "ggml.h"
#include "ggml-backend.h"
#include "ggml-rpc.h"

// Collect the non-CPU (Metal on iPad) devices to serve, mirroring
// rpc-server.cpp's get_devices(): keep accelerators; fall back to CPU only if
// none. Writes up to `cap` devices into `out`, returns how many.
static size_t collect_devices(ggml_backend_dev_t *out, size_t cap) {
    size_t n = 0;
    for (size_t i = 0; i < ggml_backend_dev_count() && n < cap; i++) {
        ggml_backend_dev_t dev = ggml_backend_dev_get(i);
        if (ggml_backend_dev_type(dev) != GGML_BACKEND_DEVICE_TYPE_CPU) {
            out[n++] = dev;
        }
    }
    if (n == 0) {  // no accelerator — fall back to CPU device(s)
        for (size_t i = 0; i < ggml_backend_dev_count() && n < cap; i++) {
            out[n++] = ggml_backend_dev_get(i);
        }
    }
    return n;
}

int rpc_shim_is_metal_available(void) {
    for (size_t i = 0; i < ggml_backend_dev_count(); i++) {
        if (ggml_backend_dev_type(ggml_backend_dev_get(i)) != GGML_BACKEND_DEVICE_TYPE_CPU) {
            return 1;
        }
    }
    return 0;
}

int rpc_shim_start(const char *endpoint,
                   unsigned long long free_bytes,
                   unsigned long long total_bytes) {
    (void)free_bytes;            // b9789 self-reports device memory; budget is
    (void)total_bytes;           // enforced via /stats, not here. See header.
    ggml_backend_dev_t devices[8];
    size_t n = collect_devices(devices, 8);
    if (n == 0) return 1;        // nothing to serve
    // n_threads = 0 -> ggml picks a default. cache_dir = NULL (no tensor cache
    // in v1). BLOCKS here until the server stops.
    ggml_backend_rpc_start_server(endpoint, NULL, /*n_threads=*/0, n, devices);
    return 0;
}

#ifndef RPC_SHIM_H
#define RPC_SHIM_H
#ifdef __cplusplus
extern "C" {
#endif

// Starts the llama.cpp RPC server on `endpoint` (e.g. "0.0.0.0:50052"),
// serving the device's non-CPU (Metal) backend. BLOCKS until the server stops
// — call from a background thread. Non-zero = no servable device / init failed.
// free_bytes/total_bytes are advisory only: b9789's start_server has no memory
// params (the server self-reports device memory); kept for forward-compat and
// currently unused. The donation budget is enforced via /stats, not here.
int rpc_shim_start(const char *endpoint,
                   unsigned long long free_bytes,
                   unsigned long long total_bytes);

// 1 if a Metal device/backend is available on this device.
int rpc_shim_is_metal_available(void);

#ifdef __cplusplus
}
#endif
#endif

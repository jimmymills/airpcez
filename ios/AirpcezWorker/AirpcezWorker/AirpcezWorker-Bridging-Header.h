//
//  AirpcezWorker-Bridging-Header.h
//  Exposes the C RPC shim to Swift. Set this file as the target's
//  "Objective-C Bridging Header" (SWIFT_OBJC_BRIDGING_HEADER) so Swift can call
//  rpc_shim_start / rpc_shim_is_metal_available without a module import.
//
//  rpc_shim.h sits beside this file in the flat synced-folder layout (same dir).
//  ggml headers are provided by the linked xcframework — no extra Header Search
//  Paths entry is needed for them.
//

#include "rpc_shim.h"

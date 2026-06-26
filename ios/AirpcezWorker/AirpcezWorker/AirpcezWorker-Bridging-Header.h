//
//  AirpcezWorker-Bridging-Header.h
//  Exposes the C RPC shim to Swift. Set this file as the target's
//  "Objective-C Bridging Header" (SWIFT_OBJC_BRIDGING_HEADER) so Swift can call
//  rpc_shim_start / rpc_shim_is_metal_available without a module import.
//
//  Requires the target's Header Search Paths to include:
//    $(SRCROOT)/Sources/RpcShim/include
//

#include "rpc_shim.h"

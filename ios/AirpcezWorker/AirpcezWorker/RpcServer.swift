import Foundation
import Combine   // ObservableObject / @Published live in Combine
// The C shim (rpc_shim_start / rpc_shim_is_metal_available) is exposed to Swift via the
// app target's bridging header (AirpcezWorker-Bridging-Header.h) — no module import needed.

@MainActor
final class RpcServer: ObservableObject {
    @Published private(set) var isListening = false
    @Published private(set) var lastError: String?

    func start(endpoint: String, freeBytes: UInt64, totalBytes: UInt64) {
        guard !isListening else { return }
        guard rpc_shim_is_metal_available() == 1 else {
            lastError = "Metal backend unavailable"; return
        }
        isListening = true
        lastError = nil
        let t = Thread {
            let rc = endpoint.withCString { ep in
                rpc_shim_start(ep, freeBytes, totalBytes)
            }
            Task { @MainActor in
                self.isListening = false
                if rc != 0 { self.lastError = "rpc_shim_start failed (\(rc))" }
            }
        }
        t.stackSize = 4 << 20
        t.start()
    }
}

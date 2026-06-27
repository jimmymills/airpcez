import Foundation
import UIKit

enum SystemMemory {
    /// Bytes this app may still allocate before iOS jetsam-kills it (post-entitlement).
    static func availableBytes() -> UInt64 { UInt64(os_proc_available_memory()) }
    static func physicalBytes() -> UInt64 { ProcessInfo.processInfo.physicalMemory }
}

// TODO(swift6): the /stats provider closure reads MainActor state (rpc.isListening, Budget.miB) off NWConnection's background queue; hop to MainActor (or snapshot via a Sendable type) when adopting Swift 6 strict concurrency.
func sampleStats(running: Bool, budgetMiB: UInt64) -> NodeStats {
    let mib: UInt64 = 1024 * 1024
    let ramTotal = SystemMemory.physicalBytes() / mib
    let availMiB = SystemMemory.availableBytes() / mib
    // Donation budget is the single source of truth: vram_total == budget.
    // vram_free tracks remaining headroom, capped at the budget (mirrors macOS).
    let vramFree = min(budgetMiB, availMiB)
    let device = DeviceStats(
        name: "MTL0", kind: "metal",
        vram_total_mib: budgetMiB, vram_free_mib: vramFree,
        reliable: budgetMiB > 0 && vramFree <= budgetMiB)
    return NodeStats(
        name: UIDeviceName.current,
        role: "worker",
        ram_total_mib: ramTotal,
        ram_free_mib: availMiB,
        cpu_logical: UInt32(ProcessInfo.processInfo.processorCount),
        devices: [device],
        rpc_endpoint: "0.0.0.0:50052",
        binary_version: LlamaVersion.tag,
        running: running,
        sampled_at_unix: UInt64(Date().timeIntervalSince1970))
}

enum UIDeviceName {
    // Cached once at first access — UIDevice.current.name is main-thread-affined; we
    // avoid re-reading it from the background /stats handler on every poll.
    static let current: String = {
        let name = UIDevice.current.name
        return name.isEmpty ? "ipad-worker" : name
    }()
}

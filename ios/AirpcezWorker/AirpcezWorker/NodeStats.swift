import Foundation

struct DeviceStats: Encodable {
    let name: String
    let kind: String          // "metal"
    let vram_total_mib: UInt64
    let vram_free_mib: UInt64
    let reliable: Bool
}

struct NodeStats: Encodable {
    let name: String
    let role: String          // "worker"
    let ram_total_mib: UInt64
    let ram_free_mib: UInt64
    let cpu_logical: UInt32
    let devices: [DeviceStats]
    let rpc_endpoint: String?  // "0.0.0.0:50052" — host rewrites the IP
    let binary_version: String?
    let running: Bool
    let sampled_at_unix: UInt64
}

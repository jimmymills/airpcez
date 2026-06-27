import Foundation

/// Single source of truth for the memory-donation budget.
/// Backed by UserDefaults; defaults to ~80 % of the process's available bytes
/// at first read (i.e. after the increased-memory entitlement has taken effect).
enum Budget {
    private static let key = "donation_budget_mib"

    /// Default ≈ 80 % of available memory at first launch (post-entitlement headroom).
    static func defaultMiB() -> UInt64 {
        let availMiB = SystemMemory.availableBytes() / (1024 * 1024)
        return UInt64(Double(availMiB) * 0.8)
    }

    static var miB: UInt64 {
        get {
            let v = UInt64(UserDefaults.standard.integer(forKey: key))
            return v == 0 ? defaultMiB() : v
        }
        set { UserDefaults.standard.set(Int(newValue), forKey: key) }
    }
}

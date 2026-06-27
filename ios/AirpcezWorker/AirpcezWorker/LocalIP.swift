import Foundation

enum LocalIP {
    /// First IPv4 on en0 (Wi-Fi). nil if unavailable.
    static var en0: String? {
        var addr: String?
        var ifaddr: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&ifaddr) == 0, let first = ifaddr else { return nil }
        var ptr = first
        while true {
            let flags = Int32(ptr.pointee.ifa_flags)
            let family = ptr.pointee.ifa_addr.pointee.sa_family
            if family == UInt8(AF_INET), (flags & IFF_LOOPBACK) == 0,
               String(cString: ptr.pointee.ifa_name) == "en0" {
                var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
                getnameinfo(ptr.pointee.ifa_addr, socklen_t(ptr.pointee.ifa_addr.pointee.sa_len),
                            &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST)
                addr = String(cString: host)
                break
            }
            guard let next = ptr.pointee.ifa_next else { break }
            ptr = next
        }
        freeifaddrs(ifaddr)
        return addr
    }
}

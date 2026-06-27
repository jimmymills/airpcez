import Foundation
import Network

/// Serves exactly one route: `GET /stats` -> NodeStats JSON. Single-user LAN, no auth.
final class StatsHTTPServer {
    private var listener: NWListener?
    private var provider: (() -> NodeStats)?

    func start(port: UInt16, statsProvider: @escaping () -> NodeStats) {
        provider = statsProvider
        let params = NWParameters.tcp
        listener = try? NWListener(using: params, on: NWEndpoint.Port(rawValue: port)!)
        listener?.newConnectionHandler = { [weak self] conn in self?.handle(conn) }
        listener?.start(queue: .global(qos: .userInitiated))
    }

    func stop() { listener?.cancel(); listener = nil }

    private func handle(_ conn: NWConnection) {
        conn.start(queue: .global(qos: .userInitiated))
        conn.receive(minimumIncompleteLength: 1, maximumLength: 8192) { [weak self] data, _, _, _ in
            let req = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            let body = self?.responseBody(for: req) ?? Data("{}".utf8)
            let header = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n" +
                         "Content-Length: \(body.count)\r\nConnection: close\r\n\r\n"
            var out = Data(header.utf8); out.append(body)
            conn.send(content: out, completion: .contentProcessed { _ in conn.cancel() })
        }
    }

    private func responseBody(for request: String) -> Data {
        guard request.hasPrefix("GET /stats") else { return Data("{}".utf8) }
        guard let stats = provider?() else { return Data("{}".utf8) }
        let enc = JSONEncoder()
        return (try? enc.encode(stats)) ?? Data("{}".utf8)
    }
}

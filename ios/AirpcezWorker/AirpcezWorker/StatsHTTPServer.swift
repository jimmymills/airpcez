import Foundation
import Network
import Combine

/// Serves exactly one route: `GET /stats` -> NodeStats JSON. Single-user LAN, no auth.
final class StatsHTTPServer: ObservableObject {
    private var listener: NWListener?
    private var provider: (() -> NodeStats)?
    @Published private(set) var lastError: String?

    func start(port: UInt16, statsProvider: @escaping () -> NodeStats) {
        provider = statsProvider
        let params = NWParameters.tcp
        do {
            listener = try NWListener(using: params, on: NWEndpoint.Port(rawValue: port)!)
        } catch {
            lastError = "Failed to create /stats listener on :\(port): \(error.localizedDescription)"
            return
        }
        listener?.stateUpdateHandler = { [weak self] state in
            switch state {
            case .failed(let err):
                DispatchQueue.main.async { self?.lastError = "/stats listener failed: \(err.localizedDescription)" }
            case .waiting(let err):
                DispatchQueue.main.async { self?.lastError = "/stats listener waiting: \(err.localizedDescription)" }
            default:
                break
            }
        }
        listener?.newConnectionHandler = { [weak self] conn in self?.handle(conn) }
        listener?.start(queue: .global(qos: .userInitiated))
    }

    func stop() { listener?.cancel(); listener = nil }

    private func handle(_ conn: NWConnection) {
        conn.start(queue: .global(qos: .userInitiated))
        conn.receive(minimumIncompleteLength: 1, maximumLength: 8192) { [weak self] data, _, _, _ in
            let req = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            if req.hasPrefix("GET /stats") {
                let body = self?.responseBody() ?? Data("{}".utf8)
                let header = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n" +
                             "Content-Length: \(body.count)\r\nConnection: close\r\n\r\n"
                var out = Data(header.utf8); out.append(body)
                conn.send(content: out, completion: .contentProcessed { _ in conn.cancel() })
            } else {
                let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                conn.send(content: Data(response.utf8), completion: .contentProcessed { _ in conn.cancel() })
            }
        }
    }

    private func responseBody() -> Data {
        guard let stats = provider?() else { return Data("{}".utf8) }
        let enc = JSONEncoder()
        return (try? enc.encode(stats)) ?? Data("{}".utf8)
    }
}

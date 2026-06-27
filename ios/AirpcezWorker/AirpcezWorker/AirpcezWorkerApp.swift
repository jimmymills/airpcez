import SwiftUI

final class AppServers { static let shared = AppServers(); let http = StatsHTTPServer() }

@main
struct AirpcezWorkerApp: App {
    @StateObject private var rpc = RpcServer()
    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(rpc)
                .onAppear {
                    UIApplication.shared.isIdleTimerDisabled = true   // keep-awake
                    let budgetMiB: UInt64 = 6 * 1024   // M1 placeholder; slider in M4
                    rpc.start(endpoint: "0.0.0.0:50052",
                              freeBytes: budgetMiB * 1024 * 1024,
                              totalBytes: budgetMiB * 1024 * 1024)
                    AppServers.shared.http.start(port: 8675) {
                        sampleStats(running: rpc.isListening, budgetMiB: budgetMiB)
                    }
                }
        }
    }
}

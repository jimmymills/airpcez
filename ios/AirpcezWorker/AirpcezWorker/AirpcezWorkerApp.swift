import SwiftUI

final class AppServers { static let shared = AppServers(); let http = StatsHTTPServer() }

@main
struct AirpcezWorkerApp: App {
    @StateObject private var rpc = RpcServer()
    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(rpc)
                .environmentObject(AppServers.shared.http)
                .onAppear {
                    UIApplication.shared.isIdleTimerDisabled = true   // keep-awake
                    _ = UIDeviceName.current   // warm the cached name on MAIN before the background /stats handler reads it
                    let budgetMiB = Budget.miB   // live from UserDefaults (M4+)
                    rpc.start(endpoint: "0.0.0.0:50052",
                              freeBytes: budgetMiB * 1024 * 1024,
                              totalBytes: budgetMiB * 1024 * 1024)
                    AppServers.shared.http.start(port: 8675) {
                        sampleStats(running: rpc.isListening, budgetMiB: Budget.miB)
                    }
                }
        }
    }
}

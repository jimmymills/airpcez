import SwiftUI

@main
struct AirpcezWorkerApp: App {
    @StateObject private var rpc = RpcServer()
    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(rpc)
                .onAppear {
                    UIApplication.shared.isIdleTimerDisabled = true   // keep-awake
                    // Hardcoded budget for M1; replaced by the slider in M4.
                    let budget: UInt64 = 6 << 30   // 6 GiB, conservative
                    rpc.start(endpoint: "0.0.0.0:50052", freeBytes: budget, totalBytes: budget)
                }
        }
    }
}

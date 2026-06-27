import SwiftUI
import Combine
import Darwin   // exit(0) for Restart

struct ContentView: View {
    @EnvironmentObject var rpc: RpcServer
    @EnvironmentObject var http: StatsHTTPServer
    @Environment(\.scenePhase) private var scenePhase
    @State private var thermalState = ProcessInfo.processInfo.thermalState

    // Initialise from persisted budget; slider range is 1–11 GiB in 0.5 steps.
    @State private var budgetGiB: Double = Double(Budget.miB) / 1024.0

    var body: some View {
        VStack(spacing: 16) {
            Text("airpcez worker").font(.title.bold())
            Label(rpc.isListening ? "RPC listening on :50052" : "stopped",
                  systemImage: rpc.isListening ? "antenna.radiowaves.left.and.right" : "xmark.circle")
                .foregroundStyle(rpc.isListening ? .green : .secondary)
            if let e = rpc.lastError { Text(e).foregroundStyle(.red).font(.footnote) }
            if let e = http.lastError { Text(e).foregroundStyle(.red).font(.footnote) }
            Text("llama.cpp \(LlamaVersion.tag)").font(.caption).foregroundStyle(.secondary)
            Text("Add in cockpit:  \(LocalIP.en0 ?? "—"):8675")
                .font(.callout.monospaced()).textSelection(.enabled)

            Divider()

            // Donation-budget slider — writes Budget.miB live; /stats reads it each poll.
            VStack(spacing: 6) {
                Slider(value: $budgetGiB, in: 1...11, step: 0.5)
                    .onChange(of: budgetGiB) { _, newVal in
                        Budget.miB = UInt64(newVal * 1024)
                    }
                Text(String(format: "Donate %.1f GiB to cluster", budgetGiB))
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }

            // Thermal warning — reactive via NSProcessInfoThermalStateDidChange notification.
            if thermalState.rawValue >= ProcessInfo.ThermalState.serious.rawValue {
                Label("Thermal throttling — performance reduced",
                      systemImage: "thermometer.high")
                    .foregroundStyle(.orange)
            }

            // Foreground warning
            if scenePhase != .active {
                Label("App backgrounded — RPC will stop",
                      systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
            }

            Divider()

            // Restart = quit the process; the blocking rpc_shim_start thread cannot be
            // torn down in-process without re-binding :50052 on a still-running thread.
            Button(role: .destructive) {
                exit(0)
            } label: {
                Label("Restart worker — closes the app; reopen from the Home Screen",
                      systemImage: "arrow.clockwise")
            }
            .buttonStyle(.bordered)
        }
        .padding()
        .onReceive(NotificationCenter.default.publisher(for: ProcessInfo.thermalStateDidChangeNotification)) { _ in
            thermalState = ProcessInfo.processInfo.thermalState
        }
    }
}

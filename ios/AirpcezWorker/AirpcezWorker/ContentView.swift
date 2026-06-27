import SwiftUI

struct ContentView: View {
    @EnvironmentObject var rpc: RpcServer
    var body: some View {
        VStack(spacing: 16) {
            Text("airpcez worker").font(.title.bold())
            Label(rpc.isListening ? "RPC listening on :50052" : "stopped",
                  systemImage: rpc.isListening ? "antenna.radiowaves.left.and.right" : "xmark.circle")
                .foregroundStyle(rpc.isListening ? .green : .secondary)
            if let e = rpc.lastError { Text(e).foregroundStyle(.red).font(.footnote) }
            Text("llama.cpp \(LlamaVersion.tag)").font(.caption).foregroundStyle(.secondary)
            Text("Add in cockpit:  \(LocalIP.en0 ?? "—"):8675")
                .font(.callout.monospaced()).textSelection(.enabled)
        }.padding()
    }
}

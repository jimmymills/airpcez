# airpcez iPad Worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a native iPadOS app that joins the airpcez cluster as a llama.cpp `rpc-server` + Metal worker, donating M5 iPad Pro unified memory as VRAM so the host can fit bigger models.

**Architecture:** A thin SwiftUI app embeds llama.cpp (ggml + Metal + RPC) as an iOS `xcframework`. A C shim starts the RPC server in-process on a Metal backend; a dependency-free Swift HTTP server answers `GET /stats` in the exact `NodeStats` JSON shape the existing host poller expects. No changes to `airpcez-core` semantics — the iPad is just another worker the host offloads layers to via `--tensor-split`.

**Tech Stack:** Swift / SwiftUI, Network.framework (HTTP), Metal, C interop (clang module), llama.cpp (`xcframework`), Xcode (beta matching iPadOS 27 Beta 2), Rust (existing repo, golden contract test only).

## Global Constraints

- **Goal is capacity, not speed.** RPC is latency-bound; WiFi is slower than the gigabit LAN. The iPad lets a bigger model *fit*; it does not make tokens faster.
- **No `airpcez-core` semantic changes.** The iPad conforms to the existing wire contract; it does not require Rust-side schema edits beyond an additive test fixture.
- **Version pin:** the iPad `xcframework` is built from the **same llama.cpp tag the host runs** (`b9789` today). The reported `binary_version` MUST equal the host's (format `"b<N>"`, e.g. `"b9789"`).
- **Donation budget governs placement via `/stats`:** one MiB value sets the `/stats` device `vram_total_mib`, which the host planner turns into `--tensor-split` — so the budget controls how many layers land on the iPad. Note: llama.cpp `b9789`'s `ggml_backend_rpc_start_server` has **no** `free_mem`/`total_mem` params (the RPC server self-reports the Metal device's memory), so the budget cannot be enforced at the RPC layer — report a *conservative* budget in `/stats` so the planner under-fills against jetsam.
- **Ports:** `/stats` HTTP on `8675`; llama.cpp RPC on `50052` (mirrors the cluster defaults).
- **`/stats` shape (verbatim from `airpcez_core::model::NodeStats`):** `name: String`, `role: "worker"`, `ram_total_mib: u64`, `ram_free_mib: u64`, `cpu_logical: u32`, `devices: [DeviceStats]`, `rpc_endpoint: "0.0.0.0:50052"` (host rewrites the IP), `binary_version: "b<N>"`, `running: bool`, `sampled_at_unix: u64`. `DeviceStats` = `name: String`, `kind: "metal"`, `vram_total_mib: u64`, `vram_free_mib: u64`, `reliable: bool`. All keys are snake_case; enums serialize lowercase.
- **Lifecycle:** foreground + keep-awake (idle timer disabled), device plugged in. No background serving.
- **Neural Engine is out of scope.** llama.cpp has no ggml ANE backend; the iPad contributes GPU + unified memory only.
- **Signing:** team provisioning profile + `com.apple.developer.kernel.increased-memory-limit` + `com.apple.developer.kernel.extended-virtual-addressing` entitlements (paid dev account).

---

## Milestone M1 — Prove `rpc-server` + Metal runs on the device (de-risk first)

This is the one real unknown. Nothing else matters until the host can connect to an RPC server running on the iPad's GPU.

### Task 1: Repo scaffold + llama.cpp iOS xcframework build script

**Files:**
- Create: `scripts/build-llama-ios-xcframework.sh`
- Create: `ios/AirpcezWorker/.gitignore`
- Modify: `.gitignore` (root — ignore iOS build artifacts)

**Interfaces:**
- Produces: `ios/AirpcezWorker/Frameworks/llama.xcframework` (device arm64 + simulator arm64), and `ios/AirpcezWorker/Generated/LlamaVersion.swift` containing `enum LlamaVersion { static let tag = "b9789" }`.

- [ ] **Step 1: Create the build script**

Create `scripts/build-llama-ios-xcframework.sh`:

```bash
#!/usr/bin/env bash
# Builds llama.cpp (ggml + Metal + RPC) as an iOS xcframework pinned to the host's tag.
# Re-run whenever the cluster's llama.cpp version moves. Keep TAG == the host's build.
set -euo pipefail

TAG="${LLAMA_TAG:-b9789}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/.llama-ios-build"
OUT="$ROOT/ios/AirpcezWorker/Frameworks"
GEN="$ROOT/ios/AirpcezWorker/Generated"

rm -rf "$WORK" && mkdir -p "$WORK" "$OUT" "$GEN"
git clone --depth 1 --branch "$TAG" https://github.com/ggml-org/llama.cpp "$WORK/llama.cpp"
cd "$WORK/llama.cpp"

# Common CMake flags: enable Metal + RPC, disable everything that won't link on iOS.
# The OFF set mirrors llama.cpp's own build-xcframework.sh (canonical reference); we add
# GGML_RPC (upstream doesn't). Every executable-producing target MUST be OFF — they need an
# iOS bundle id / signing and break the static-lib build. APP (the unified "llama" binary)
# and SERVER default ON for a standalone build, so disabling EXAMPLES/TOOLS/TESTS alone is
# NOT enough — APP and SERVER must be set explicitly (this was the b9789 build failure).
common_flags=(
  -DGGML_METAL=ON -DGGML_RPC=ON
  -DGGML_METAL_EMBED_LIBRARY=ON          # bake the .metallib into the binary (no bundle path)
  -DGGML_METAL_USE_BF16=ON               # bf16 Metal kernels (matches upstream xcframework build)
  -DLLAMA_BUILD_APP=OFF -DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_COMMON=OFF
  -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_TOOLS=OFF
  -DLLAMA_CURL=OFF
  -DBUILD_SHARED_LIBS=OFF
  -DCMAKE_BUILD_TYPE=Release
)

build_one () { # $1 = sysroot name, $2 = platform dir, $3 = arch
  cmake -B "build-$1" -G Xcode \
    -DCMAKE_SYSTEM_NAME=iOS \
    -DCMAKE_OSX_SYSROOT="$1" \
    -DCMAKE_OSX_ARCHITECTURES="$3" \
    -DCMAKE_OSX_DEPLOYMENT_TARGET=17.0 \
    "${common_flags[@]}"
  cmake --build "build-$1" --config Release -- -quiet
}

build_one iphoneos device arm64
build_one iphonesimulator simulator arm64

# Collect the static libs (ggml*, llama) per slice into a single lib + headers.
package_slice () { # $1 = build dir, $2 = dest dir
  mkdir -p "$2/lib" "$2/include"
  find "$1" -name '*.a' -exec cp {} "$2/lib/" \;
  cp -R src/*.h include/*.h ggml/include/*.h "$2/include/" 2>/dev/null || true
  libtool -static -o "$2/libllama_full.a" "$2"/lib/*.a
}
package_slice "build-iphoneos" "$WORK/slice-device"
package_slice "build-iphonesimulator" "$WORK/slice-sim"

rm -rf "$OUT/llama.xcframework"
xcodebuild -create-xcframework \
  -library "$WORK/slice-device/libllama_full.a" -headers "$WORK/slice-device/include" \
  -library "$WORK/slice-sim/libllama_full.a"    -headers "$WORK/slice-sim/include" \
  -output "$OUT/llama.xcframework"

printf 'enum LlamaVersion { static let tag = "%s" }\n' "$TAG" > "$GEN/LlamaVersion.swift"
echo "Built llama.xcframework ($TAG) -> $OUT"
```

- [ ] **Step 2: Make it executable and add ignores**

Create `ios/AirpcezWorker/.gitignore`:

```
build/
DerivedData/
*.xcuserstate
xcuserdata/
Frameworks/llama.xcframework/
Generated/LlamaVersion.swift
```

Append to the root `.gitignore`:

```
/.llama-ios-build/
```

Run: `chmod +x scripts/build-llama-ios-xcframework.sh`

- [ ] **Step 3: Build the xcframework**

Run: `LLAMA_TAG=b9789 ./scripts/build-llama-ios-xcframework.sh`
Expected: ends with `Built llama.xcframework (b9789) -> .../Frameworks`, and `ios/AirpcezWorker/Frameworks/llama.xcframework/` contains `ios-arm64/` and `ios-arm64-simulator/` slices.

If a CMake flag is rejected by this tag (flags drift across releases), read that tag's `CMakeLists.txt`/`ggml/CMakeLists.txt` for the current option names and adjust — do not guess.

- [ ] **Step 4: Commit**

```bash
git add scripts/build-llama-ios-xcframework.sh ios/AirpcezWorker/.gitignore .gitignore
git commit -m "build(ipad): llama.cpp iOS xcframework build script (Metal+RPC, version-pinned)"
```

### Task 2: C shim that starts the RPC server on a Metal backend

**Files:**
- Create: `ios/AirpcezWorker/Sources/RpcShim/include/rpc_shim.h`
- Create: `ios/AirpcezWorker/Sources/RpcShim/rpc_shim.c`
- Create: `ios/AirpcezWorker/Sources/RpcShim/include/module.modulemap`

**Interfaces:**
- Produces (C, imported into Swift as module `RpcShim`):
  - `int rpc_shim_start(const char *endpoint, unsigned long long free_bytes, unsigned long long total_bytes);` — inits the Metal backend and starts the RPC server; **blocks** (call on a background thread). Returns non-zero on init failure.
  - `int rpc_shim_is_metal_available(void);` — 1 if a Metal device/backend initialised.

- [ ] **Step 1: Upstream API (confirmed against tag `b9789`)**

Verified from `ggml/include/ggml-rpc.h` and `tools/rpc/rpc-server.cpp` at `b9789`:
- Server entry: `void ggml_backend_rpc_start_server(const char *endpoint, const char *cache_dir, size_t n_threads, size_t n_devices, ggml_backend_dev_t *devices);` — **no** `free_mem`/`total_mem` params; the server self-reports each device's memory.
- Device selection (from `rpc-server.cpp`'s `get_devices()`): enumerate `ggml_backend_dev_get(i)` for `i < ggml_backend_dev_count()`, keep devices whose `ggml_backend_dev_type(dev) != GGML_BACKEND_DEVICE_TYPE_CPU` (Metal on iPad); fall back to CPU only if none.

If you re-clone at a *different* tag, re-confirm this signature in `ggml-rpc.h` and adapt — the API has changed across releases.

- [ ] **Step 2: Write the shim header**

Create `ios/AirpcezWorker/Sources/RpcShim/include/rpc_shim.h`:

```c
#ifndef RPC_SHIM_H
#define RPC_SHIM_H
#ifdef __cplusplus
extern "C" {
#endif

// Starts the llama.cpp RPC server on `endpoint` (e.g. "0.0.0.0:50052"),
// serving the device's non-CPU (Metal) backend. BLOCKS until the server stops
// — call from a background thread. Non-zero = no servable device / init failed.
// free_bytes/total_bytes are advisory only: b9789's start_server has no memory
// params (the server self-reports device memory); kept for forward-compat and
// currently unused. The donation budget is enforced via /stats, not here.
int rpc_shim_start(const char *endpoint,
                   unsigned long long free_bytes,
                   unsigned long long total_bytes);

// 1 if a Metal device/backend is available on this device.
int rpc_shim_is_metal_available(void);

#ifdef __cplusplus
}
#endif
#endif
```

- [ ] **Step 3: Write the shim implementation**

Create `ios/AirpcezWorker/Sources/RpcShim/rpc_shim.c`:

```c
#include "rpc_shim.h"
#include "ggml.h"
#include "ggml-backend.h"
#include "ggml-rpc.h"

// Collect the non-CPU (Metal on iPad) devices to serve, mirroring
// rpc-server.cpp's get_devices(): keep accelerators; fall back to CPU only if
// none. Writes up to `cap` devices into `out`, returns how many.
static size_t collect_devices(ggml_backend_dev_t *out, size_t cap) {
    size_t n = 0;
    for (size_t i = 0; i < ggml_backend_dev_count() && n < cap; i++) {
        ggml_backend_dev_t dev = ggml_backend_dev_get(i);
        if (ggml_backend_dev_type(dev) != GGML_BACKEND_DEVICE_TYPE_CPU) {
            out[n++] = dev;
        }
    }
    if (n == 0) {  // no accelerator — fall back to CPU device(s)
        for (size_t i = 0; i < ggml_backend_dev_count() && n < cap; i++) {
            out[n++] = ggml_backend_dev_get(i);
        }
    }
    return n;
}

int rpc_shim_is_metal_available(void) {
    for (size_t i = 0; i < ggml_backend_dev_count(); i++) {
        if (ggml_backend_dev_type(ggml_backend_dev_get(i)) != GGML_BACKEND_DEVICE_TYPE_CPU) {
            return 1;
        }
    }
    return 0;
}

int rpc_shim_start(const char *endpoint,
                   unsigned long long free_bytes,
                   unsigned long long total_bytes) {
    (void)free_bytes;            // b9789 self-reports device memory; budget is
    (void)total_bytes;           // enforced via /stats, not here. See header.
    ggml_backend_dev_t devices[8];
    size_t n = collect_devices(devices, 8);
    if (n == 0) return 1;        // nothing to serve
    // n_threads = 0 -> ggml picks a default. cache_dir = NULL (no tensor cache
    // in v1). BLOCKS here until the server stops.
    ggml_backend_rpc_start_server(endpoint, NULL, /*n_threads=*/0, n, devices);
    return 0;
}
```

> **Static-linking gotcha (read before M1 build):** with `BUILD_SHARED_LIBS=OFF`,
> the Metal backend registers itself via a static constructor whose object file
> the linker can dead-strip (nothing references it directly), making
> `ggml_backend_dev_count()` return only CPU / zero. Task 3 sets `-all_load` (or
> `-force_load` on `libllama_full.a`) in the app target's **Other Linker Flags**
> to keep the backend registrars. If Metal "isn't found" on-device, this flag is
> the first thing to check.

- [ ] **Step 4: Write the module map so Swift can import it**

Create `ios/AirpcezWorker/Sources/RpcShim/include/module.modulemap`:

```
module RpcShim {
    header "rpc_shim.h"
    export *
}
```

- [ ] **Step 5: Commit**

```bash
git add ios/AirpcezWorker/Sources/RpcShim
git commit -m "feat(ipad): C shim to start llama.cpp RPC server on a Metal backend"
```

### Task 3: Minimal SwiftUI app that starts the RPC server on launch; verify host connects

**Files:**
- Create: `ios/AirpcezWorker/AirpcezWorker.xcodeproj` (via Xcode)
- Create: `ios/AirpcezWorker/Sources/App/AirpcezWorkerApp.swift`
- Create: `ios/AirpcezWorker/Sources/App/RpcServer.swift`
- Create: `ios/AirpcezWorker/Sources/App/ContentView.swift`

**Interfaces:**
- Consumes: `RpcShim.rpc_shim_start`, `LlamaVersion.tag`.
- Produces: `final class RpcServer: ObservableObject` with `@Published var isListening: Bool`, `func start(endpoint: String, freeBytes: UInt64, totalBytes: UInt64)`, `func stop()`.

- [ ] **Step 1: Create the Xcode project**

In Xcode (matching iPadOS 27 Beta 2 — see Development Environment in the spec): New Project → iOS App → name `AirpcezWorker`, interface SwiftUI, language Swift, save into `ios/AirpcezWorker/`. Then:
- Add `Frameworks/llama.xcframework` to the target (General → Frameworks, Libraries → **"Do Not Embed"** — it wraps *static* libs, so it's linked into the binary, not embedded as a dynamic framework).
- Add `NSLocalNetworkUsageDescription` to Info.plist (e.g. "Joins the airpcez cluster over the local network"). A listening RPC socket triggers iOS Local Network privacy; without this the host's connection is silently blocked.
- Delete the two stub files Xcode auto-generates (`AirpcezWorkerApp.swift`, `ContentView.swift`) before adding ours — same type names would collide (duplicate `@main`).
- Add `Sources/RpcShim/rpc_shim.c` to the target's **Compile Sources**. Expose the shim to Swift via a **bridging header**: set Build Settings → "Objective-C Bridging Header" to `Sources/App/AirpcezWorker-Bridging-Header.h` (it `#include`s `rpc_shim.h`), and add `$(SRCROOT)/Sources/RpcShim/include` to **Header Search Paths**. (A bridging header is far more reliable than a module map in a hand-built app target — RpcServer.swift therefore does NOT `import RpcShim`.)
- Add `Generated/LlamaVersion.swift` to the target.
- Link required system frameworks: `Metal`, `MetalKit`, `Accelerate`, `Foundation`.
- **Build Settings → Other Linker Flags: add `-all_load -lc++`.** `-all_load` keeps the statically-linked Metal backend's self-registration from being dead-stripped (else `ggml_backend_dev_count()` sees no Metal device — the #1 silent M1 failure). `-lc++` links the C++ standard library that llama.cpp/ggml need (the target is pure Swift+C, so Xcode won't auto-link libc++ → undefined `std::` symbols without it).
- **Xcode 16 note:** sources live *inside* the target's synced folder (`AirpcezWorker/AirpcezWorker/`), flat, so they auto-compile — no "Add Files." Bridging header path is `AirpcezWorker/AirpcezWorker-Bridging-Header.h`; `rpc_shim.h` sits beside it and `rpc_shim.c` so includes resolve same-dir; ggml headers come from the linked xcframework.

- [ ] **Step 2: Write the RPC server wrapper**

Create `ios/AirpcezWorker/Sources/App/RpcServer.swift`:

```swift
import Foundation
import Combine   // ObservableObject / @Published live in Combine
// C shim (rpc_shim_*) is exposed via the app target's bridging header — no module import.

@MainActor
final class RpcServer: ObservableObject {
    @Published private(set) var isListening = false
    @Published private(set) var lastError: String?

    private var thread: Thread?

    func start(endpoint: String, freeBytes: UInt64, totalBytes: UInt64) {
        guard !isListening else { return }
        guard rpc_shim_is_metal_available() == 1 else {
            lastError = "Metal backend unavailable"; return
        }
        isListening = true
        lastError = nil
        let t = Thread {
            let rc = endpoint.withCString { ep in
                rpc_shim_start(ep, freeBytes, totalBytes)
            }
            Task { @MainActor in
                self.isListening = false
                if rc != 0 { self.lastError = "rpc_shim_start failed (\(rc))" }
            }
        }
        t.stackSize = 4 << 20
        t.start()
        thread = t
    }

    // v1 stop = process the server thread out of band is not graceful; the
    // server blocks. For a clean restart we tear down the whole backend by
    // killing listening state and re-launching the app's start path. See M4.
    func stop() { isListening = false }
}
```

- [ ] **Step 3: Write the app entry + a status view**

Create `ios/AirpcezWorker/Sources/App/AirpcezWorkerApp.swift`:

```swift
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
```

Create `ios/AirpcezWorker/Sources/App/ContentView.swift`:

```swift
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
        }.padding()
    }
}
```

- [ ] **Step 4: Build and run on the device**

Run on the M5 iPad Pro from Xcode (Cmd-R). Expected: app shows "RPC listening on :50052". Note the iPad's LAN IP (Settings → Wi-Fi → ⓘ).

- [ ] **Step 5: Verify the host can reach the RPC port (the de-risk gate)**

From the host machine (replace `IPAD_IP`):

Run: `nc -vz IPAD_IP 50052`
Expected: `succeeded` / `open`.

Then a real connection — launch a tiny model with only the iPad as an RPC backend:

Run (on host, in the llama.cpp build dir): `./llama-server -m <small.gguf> --rpc IPAD_IP:50052 -ngl 99 --host 0.0.0.0 --port 8080`
Expected: host log shows it connected to the RPC backend and offloaded layers; a `curl` chat completion against `:8080` returns tokens. The iPad's Metal GPU is now doing work for the cluster.

If this fails, capture the host's RPC log lines and the iPad's Xcode console before proceeding — M2–M4 all depend on this working.

- [ ] **Step 6: Commit**

```bash
git add ios/AirpcezWorker
git commit -m "feat(ipad): minimal app starts in-process RPC+Metal server on launch"
```

---

## Milestone M2 — iPad appears in the cockpit as a node

> **Layout correction (post-M1 restructure):** Xcode 16/17 uses a *synchronized folder*.
> All iOS sources — existing and new — live FLAT in `ios/AirpcezWorker/AirpcezWorker/`
> (the target's synced folder). For every task below, create/modify files there, NOT under
> `Sources/App/` or `Sources/RpcShim/` (those paths in the file lists are superseded). New
> Swift files auto-compile by being placed in that folder. `RpcServer.swift` already has
> `import Combine`. The current `AirpcezWorkerApp.swift` starts the RPC server with a
> hardcoded 6 GiB budget (`6 << 30` bytes ⇒ `budgetMiB = 6144`); M2 reuses that same value
> for `/stats` until M4 makes it a slider.

### Task 4: Stats provider + `NodeStats`-shaped Codable

**Files:**
- Create: `ios/AirpcezWorker/Sources/App/NodeStats.swift`
- Create: `ios/AirpcezWorker/Sources/App/StatsProvider.swift`

**Interfaces:**
- Produces: `struct NodeStats: Encodable` and `struct DeviceStats: Encodable` with snake_case `CodingKeys` matching the Rust structs; `func sampleStats(running: Bool, budgetMiB: UInt64) -> NodeStats`.

- [ ] **Step 1: Write the Codable mirror (snake_case keys)**

Create `ios/AirpcezWorker/Sources/App/NodeStats.swift`:

```swift
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
```

- [ ] **Step 2: Write the sampler (mirrors the macOS Apple-Silicon path)**

Create `ios/AirpcezWorker/Sources/App/StatsProvider.swift`:

```swift
import Foundation

enum SystemMemory {
    /// Bytes this app may still allocate before iOS jetsam-kills it (post-entitlement).
    static func availableBytes() -> UInt64 { UInt64(os_proc_available_memory()) }
    static func physicalBytes() -> UInt64 { ProcessInfo.processInfo.physicalMemory }
}

func sampleStats(running: Bool, budgetMiB: UInt64) -> NodeStats {
    let mib: UInt64 = 1024 * 1024
    let ramTotal = SystemMemory.physicalBytes() / mib
    let availMiB = SystemMemory.availableBytes() / mib
    // Donation budget is the single source of truth: vram_total == budget.
    // vram_free tracks remaining headroom, capped at the budget (mirrors macOS).
    let vramFree = min(budgetMiB, availMiB)
    let device = DeviceStats(
        name: "MTL0", kind: "metal",
        vram_total_mib: budgetMiB, vram_free_mib: vramFree,
        reliable: budgetMiB > 0 && vramFree <= budgetMiB)
    return NodeStats(
        name: UIDeviceName.current,
        role: "worker",
        ram_total_mib: ramTotal,
        ram_free_mib: availMiB,
        cpu_logical: UInt32(ProcessInfo.processInfo.processorCount),
        devices: [device],
        rpc_endpoint: "0.0.0.0:50052",
        binary_version: LlamaVersion.tag,
        running: running,
        sampled_at_unix: UInt64(Date().timeIntervalSince1970))
}

enum UIDeviceName {
    static var current: String {
        // Stable, human-readable; falls back to a constant if name access is restricted.
        UIDevice.current.name.isEmpty ? "ipad-worker" : UIDevice.current.name
    }
}
```
(import `UIKit` at the top of the file for `UIDevice`.)

- [ ] **Step 3: Commit**

```bash
git add ios/AirpcezWorker/Sources/App/NodeStats.swift ios/AirpcezWorker/Sources/App/StatsProvider.swift
git commit -m "feat(ipad): NodeStats Codable mirror + Apple-Silicon stats sampler"
```

### Task 5: Dependency-free `/stats` HTTP server (Network.framework)

**Files:**
- Create: `ios/AirpcezWorker/Sources/App/StatsHTTPServer.swift`
- Modify: `ios/AirpcezWorker/Sources/App/AirpcezWorkerApp.swift` (start it on launch)

**Interfaces:**
- Consumes: `sampleStats(running:budgetMiB:)`, `RpcServer.isListening`.
- Produces: `final class StatsHTTPServer` with `func start(port: UInt16, statsProvider: @escaping () -> NodeStats)` and `func stop()`.

- [ ] **Step 1: Write a minimal HTTP/1.1 responder**

Create `ios/AirpcezWorker/Sources/App/StatsHTTPServer.swift`:

```swift
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
```

- [ ] **Step 2: Start it on launch alongside the RPC server**

In `AirpcezWorkerApp.swift`, add `@StateObject private var http = StatsHTTPServer()` is not needed (it's not Observable); instead hold it in a small holder. Replace the `.onAppear` block:

```swift
.onAppear {
    UIApplication.shared.isIdleTimerDisabled = true
    let budgetMiB: UInt64 = 6 * 1024   // M1 placeholder; slider in M4
    rpc.start(endpoint: "0.0.0.0:50052",
              freeBytes: budgetMiB * 1024 * 1024,
              totalBytes: budgetMiB * 1024 * 1024)
    AppServers.shared.http.start(port: 8675) {
        sampleStats(running: rpc.isListening, budgetMiB: budgetMiB)
    }
}
```

Add a tiny holder so the server outlives the closure — create it inline in `AirpcezWorkerApp.swift`:

```swift
final class AppServers { static let shared = AppServers(); let http = StatsHTTPServer() }
```

- [ ] **Step 3: Verify `/stats` on-device**

Build & run. From the host:

Run: `curl -s http://IPAD_IP:8675/stats | python3 -m json.tool`
Expected: JSON with `"role": "worker"`, `"binary_version": "b9789"`, `"rpc_endpoint": "0.0.0.0:50052"`, one `"kind": "metal"` device with `vram_total_mib` ≈ budget.

- [ ] **Step 4: Commit**

```bash
git add ios/AirpcezWorker/Sources/App
git commit -m "feat(ipad): /stats HTTP server (Network.framework), started on launch"
```

### Task 6: Rust-side golden contract test (TDD — the one pure-testable piece)

**Files:**
- Create: `crates/airpcez/tests/fixtures/ipad_stats.json`
- Create: `crates/airpcez/tests/ipad_contract.rs`

**Interfaces:**
- Consumes: `airpcez_core::model::{NodeStats, DeviceStats, Role, DeviceKind}`.

- [ ] **Step 1: Write the failing test**

Create `crates/airpcez/tests/ipad_contract.rs`:

```rust
use airpcez_core::model::*;

// The iPad worker's /stats payload MUST deserialize into NodeStats unchanged.
// Capture is taken verbatim from `curl http://IPAD_IP:8675/stats` (Task 5).
#[test]
fn ipad_stats_payload_deserializes_into_node_stats() {
    let json = include_str!("fixtures/ipad_stats.json");
    let stats: NodeStats = serde_json::from_str(json).expect("iPad /stats must parse as NodeStats");
    assert_eq!(stats.role, Role::Worker);
    assert_eq!(stats.binary_version.as_deref(), Some("b9789"));
    assert_eq!(stats.rpc_endpoint.as_deref(), Some("0.0.0.0:50052"));
    assert_eq!(stats.devices.len(), 1);
    let d = &stats.devices[0];
    assert_eq!(d.kind, DeviceKind::Metal);
    // Donation budget is the single source of truth: free never exceeds total.
    assert!(d.vram_free_mib <= d.vram_total_mib);
    assert!(d.reliable);
}
```

- [ ] **Step 2: Add the captured fixture**

Create `crates/airpcez/tests/fixtures/ipad_stats.json` with the **actual** body captured in Task 5 Step 3 (this exact example is a valid stand-in until the real capture replaces it):

```json
{
  "name": "ipad-pro-m5",
  "role": "worker",
  "ram_total_mib": 12288,
  "ram_free_mib": 9000,
  "cpu_logical": 9,
  "devices": [
    { "name": "MTL0", "kind": "metal", "vram_total_mib": 6144, "vram_free_mib": 6000, "reliable": true }
  ],
  "rpc_endpoint": "0.0.0.0:50052",
  "binary_version": "b9789",
  "running": true,
  "sampled_at_unix": 1782460000
}
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p airpcez --test ipad_contract`
Expected: PASS. (If it fails, the Swift `CodingKeys`/values diverged from `NodeStats` — fix the Swift side, re-capture, update the fixture.)

- [ ] **Step 4: Commit**

```bash
git add crates/airpcez/tests/ipad_contract.rs crates/airpcez/tests/fixtures/ipad_stats.json
git commit -m "test(ipad): golden contract — iPad /stats deserializes into NodeStats"
```

### Task 7: Add the iPad in the cockpit + version-match check

**Files:** none (manual cluster operation; UI hint in the app).

- [ ] **Step 1: Show the copy-paste node string in the app**

In `ContentView.swift`, add below the status (use the en0 IPv4 helper):

```swift
Text("Add in cockpit:  \(LocalIP.en0 ?? "—"):8675")
    .font(.callout.monospaced()).textSelection(.enabled)
```

Add `LocalIP` (new file `ios/AirpcezWorker/Sources/App/LocalIP.swift`):

```swift
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
```

- [ ] **Step 2: Add the node and verify it appears**

In the cockpit (host UI on `:8675`), add a node with addr `IPAD_IP:8675`. Expected: the iPad appears in the node list with its Metal device and donatable VRAM; `running` reflects the RPC state. Confirm **no version-mismatch warning** (iPad `binary_version` == host). If a warning shows, rebuild the xcframework from the host's exact tag.

- [ ] **Step 3: Commit**

```bash
git add ios/AirpcezWorker/Sources/App/LocalIP.swift ios/AirpcezWorker/Sources/App/ContentView.swift
git commit -m "feat(ipad): show LAN node string for manual cockpit add"
```

---

## Milestone M3 — Real layer offload (capacity proven end-to-end)

### Task 8: Host loads a model with layers placed on the iPad

**Files:** none (cluster operation + empirical notes).

- [ ] **Step 1: Pick a model that needs the extra capacity**

Choose a model/quant that does *not* fit on the existing cluster without the iPad (the whole point). Confirm via the planner's fit verdict before adding the iPad, then add it.

- [ ] **Step 2: Plan + launch from the cockpit**

Use the cockpit's suggest/launch path so the planner sets `--tensor-split` including the iPad's Metal pool, then launch `llama-server --rpc …,IPAD_IP:50052`. Expected: pre-flight verdict flips to ✅ fits with the iPad included; launch succeeds.

- [ ] **Step 3: Generate and observe**

Run a chat completion against the host's OpenAI endpoint. Expected: tokens stream; the iPad's Xcode console / thermal indicator shows activity. Record tok/s for the record (expected: slower than solo — capacity, not speed).

- [ ] **Step 4: Note the working budget**

Record the largest donation budget that loaded without a jetsam kill (informs M4's default). No commit (operational milestone) — capture findings in the spec's testing notes or a memory.

---

## Milestone M4 — Memory unlock + UX polish

### Task 9: Entitlements + donation-budget wiring (single source of truth)

**Files:**
- Create: `ios/AirpcezWorker/AirpcezWorker.entitlements`
- Modify: target Signing & Capabilities (Xcode)
- Create: `ios/AirpcezWorker/Sources/App/Budget.swift`
- Modify: `ios/AirpcezWorker/Sources/App/AirpcezWorkerApp.swift`

**Interfaces:**
- Produces: `enum Budget { static var miB: UInt64 { get set }; static func defaultMiB() -> UInt64 }` backed by `UserDefaults`.

- [ ] **Step 1: Add the entitlements file**

Create `ios/AirpcezWorker/AirpcezWorker.entitlements`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.developer.kernel.increased-memory-limit</key>
    <true/>
    <key>com.apple.developer.kernel.extended-virtual-addressing</key>
    <true/>
</dict>
</plist>
```

In Xcode: target → Signing & Capabilities → set Code Signing Entitlements to this file; ensure the provisioning profile includes both capabilities.

- [ ] **Step 2: Persisted budget with a safe default**

Create `ios/AirpcezWorker/Sources/App/Budget.swift`:

```swift
import Foundation

enum Budget {
    private static let key = "donation_budget_mib"

    /// Default ≈ 80% of available memory at first launch (post-entitlement headroom).
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
```

- [ ] **Step 3: Feed the budget into BOTH the RPC server and /stats**

In `AirpcezWorkerApp.swift` `.onAppear`, replace the hardcoded `budgetMiB`:

```swift
let budgetMiB = Budget.miB
rpc.start(endpoint: "0.0.0.0:50052",
          freeBytes: budgetMiB * 1024 * 1024,
          totalBytes: budgetMiB * 1024 * 1024)
AppServers.shared.http.start(port: 8675) {
    sampleStats(running: rpc.isListening, budgetMiB: budgetMiB)
}
```

- [ ] **Step 4: Verify the entitlement raised the ceiling**

Build & run; print `SystemMemory.availableBytes()` on launch. Expected: meaningfully higher than the default (~few GB) cap — confirming the entitlement is active. Re-run Task 8 with a larger budget.

- [ ] **Step 5: Commit**

```bash
git add ios/AirpcezWorker/AirpcezWorker.entitlements ios/AirpcezWorker/Sources/App/Budget.swift ios/AirpcezWorker/Sources/App/AirpcezWorkerApp.swift
git commit -m "feat(ipad): increased-memory entitlements + persisted donation budget"
```

### Task 10: Budget slider + thermal/foreground warnings + Restart

**Files:**
- Modify: `ios/AirpcezWorker/Sources/App/ContentView.swift`
- Modify: `ios/AirpcezWorker/Sources/App/RpcServer.swift` (restart support)

**Interfaces:**
- Consumes: `Budget`, `ProcessInfo.thermalState`, scene phase.

- [ ] **Step 1: Restart support on RpcServer**

The RPC server thread blocks; a budget change needs a fresh server. Add an app-level restart that re-invokes `start` after marking not-listening (full graceful teardown of the upstream server is out of scope for v1 — document that Restart re-launches the listener and that a stuck state may need an app relaunch). In `RpcServer.swift` add:

```swift
func restart(endpoint: String, freeBytes: UInt64, totalBytes: UInt64) {
    stop()
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
        self?.start(endpoint: endpoint, freeBytes: freeBytes, totalBytes: totalBytes)
    }
}
```

- [ ] **Step 2: Slider + warnings UI**

In `ContentView.swift`, expand the body:

```swift
@State private var budgetGiB: Double = Double(Budget.miB) / 1024.0
@Environment(\.scenePhase) private var scenePhase

// inside VStack:
Slider(value: $budgetGiB, in: 1...11, step: 0.5) { Text("Budget") }
Text(String(format: "Donate %.1f GiB", budgetGiB)).font(.callout)
Button("Apply & Restart server") {
    let mib = UInt64(budgetGiB * 1024); Budget.miB = mib
    rpc.restart(endpoint: "0.0.0.0:50052",
                freeBytes: mib * 1024 * 1024, totalBytes: mib * 1024 * 1024)
}
if ProcessInfo.processInfo.thermalState.rawValue >= ProcessInfo.ThermalState.serious.rawValue {
    Label("Thermal throttling — performance reduced", systemImage: "thermometer.high")
        .foregroundStyle(.orange)
}
if scenePhase != .active {
    Label("App backgrounded — RPC will stop", systemImage: "exclamationmark.triangle")
        .foregroundStyle(.red)
}
```

- [ ] **Step 3: Verify on-device**

Build & run. Move the slider, tap Apply & Restart, confirm `/stats` `vram_total_mib` changes to match and the RPC server comes back. Background the app → warning shows. Put the iPad under sustained load → thermal warning appears.

- [ ] **Step 4: Commit**

```bash
git add ios/AirpcezWorker/Sources/App/ContentView.swift ios/AirpcezWorker/Sources/App/RpcServer.swift
git commit -m "feat(ipad): donation slider, restart, thermal + background warnings"
```

---

## Self-Review

**Spec coverage:**
- Capacity goal, M5 target, Metal-RPC path → M1 + M3. ✓
- Approach A (thin Swift + xcframework + reimplemented /stats + Rust golden test) → Tasks 1–6. ✓
- `/stats` contract mirroring `NodeStats` → Tasks 4–6 (+ Global Constraints). ✓
- Donation budget single source of truth → Tasks 5, 9 (fed to RPC + /stats from one value). ✓
- Entitlements / increased memory → Task 9. ✓
- Version pin + match warning → Task 1 (LlamaVersion), Task 7. ✓
- Lifecycle (foreground + keep-awake), thermals, restart, WiFi drop → Tasks 3, 10 (WiFi drop handled host-side, noted). ✓
- iPadOS 27 Beta 2 toolchain → Task 3 Step 1 note (+ spec Development Environment). ✓
- Manual node add → Task 7. ✓
- ANE out → Global Constraints. ✓
- De-risk-first milestone ordering → M1 before all else. ✓

**Placeholder scan:** The `6 GiB`/`6 * 1024` values in M1/M2 are intentional, labelled placeholders superseded by `Budget` in M4 (the de-risk milestones run before entitlements exist). The `ipad_stats.json` fixture is a valid concrete example explicitly replaced by the real capture from Task 5. No unlabelled TODOs.

**Type consistency:** `sampleStats(running:budgetMiB:)`, `RpcServer.start(endpoint:freeBytes:totalBytes:)`/`restart(...)`, `StatsHTTPServer.start(port:statsProvider:)`, `Budget.miB`, `LlamaVersion.tag`, `LocalIP.en0`, `SystemMemory.availableBytes()` are referenced consistently across tasks. Swift `CodingKeys` are snake_case to match `NodeStats`. ✓

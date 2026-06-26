# airpcez iPad Worker — Design Spec

**Date:** 2026-06-26
**Status:** Approved (brainstorming complete; ready for implementation planning)

## Overview

A native iPadOS app that joins the airpcez cluster as a **capacity-adding Metal RPC
worker**. It runs llama.cpp's `rpc-server` (Metal backend) in-process and serves a
`/stats` endpoint in the shape `airpcez-core`'s `NodeStats` expects, so the existing
host polls it, places layers on it via `--tensor-split`, and offloads tensor ops to it
over the LAN — exactly like any other worker.

The win is **capacity**: donate most of the M5 iPad Pro's ~12 GB unified memory (as
Metal/VRAM) to hold model layers that otherwise wouldn't fit. This is explicitly **not**
a speed play — RPC is latency-bound (see the project's own gigabit finding: solo `.24`
beats distributed RPC), and WiFi is slower than the gigabit LAN. Adding the iPad lets a
bigger model *fit*; it does not make tokens faster.

**Ethos:** same as airpcez itself — a thin shell wrapping the proven llama.cpp binaries.
The iPad app is a control/transport shell around the official `rpc-server` library code +
the Metal backend; we add no inference logic.

## Goals

- Make the M5 iPad Pro a first-class airpcez worker node with **no changes to
  `airpcez-core` semantics** — it appears in the cockpit and gets layers placed on it
  through the existing planner/flag-builder path.
- Donate a **tunable, large fraction** of the iPad's unified memory as Metal VRAM,
  unlocked by the `increased-memory-limit` entitlement.
- Keep the host-facing surface minimal: one `GET /stats` endpoint + one RPC port.
- Stay honest about the trade: capacity up, per-token speed down.

## Non-goals (v1)

- **Neural Engine / Core ML / MLX.** llama.cpp has no ggml ANE backend; the ANE is a poor
  fit for autoregressive LLM decode and is unreachable from the RPC path. The iPad
  contributes its **GPU + unified memory** only.
- Faster inference (the opposite is expected).
- Background / suspended serving (iOS suspends backgrounded apps → RPC dies). v1 is
  foreground + keep-awake, device plugged in.
- Auto-discovery (manual node add, consistent with airpcez's existing ethos).
- The iPad acting as **host**; multi-model hosting; host-initiated remote restart; auth
  (LAN-trusted, single user).

## Key decisions (from brainstorming)

| Decision | Choice |
|---|---|
| Primary goal | **Capacity** — fit bigger models, accept slower per-token |
| Target hardware | **M5 iPad Pro 13"**, ~12 GB unified memory |
| Compute path | **llama.cpp `rpc-server` + Metal backend** in-process (GPU + unified memory) |
| Neural Engine | **Out** — no ggml ANE backend; architecturally unsuited to LLM decode |
| Build approach | **A** — thin SwiftUI app + llama.cpp `xcframework`; reimplement the small `/stats` surface in Swift; golden contract test on the Rust side |
| Memory unlock | **Paid dev account** + `increased-memory-limit` / `extended-virtual-addressing` entitlements |
| Donation amount | **User-tunable budget slider**; governs placement via the `/stats` device VRAM (planner → `--tensor-split`). `b9789` RPC has no free/total param, so it self-reports device memory — budget is enforced in `/stats`, not at the RPC call |
| Lifecycle | **Foreground + keep-awake + plugged in** |
| Node add | **Manual** (type `ip:8675` into the cockpit) |
| Version match | iPad `xcframework` pinned to the **host's llama.cpp tag** (`b9789` today) |

## Architecture

A SwiftUI app with two LAN listeners and one compute engine, all in-process:

- **llama.cpp as an iOS `xcframework`** — ggml with the **Metal** and **RPC** backends,
  built from the **same release tag the host runs**.
- **A C shim** that enumerates ggml devices, keeps the non-CPU (Metal) one, and calls the
  RPC server entry (`ggml_backend_rpc_start_server(endpoint, cache_dir, n_threads,
  n_devices, devices)` — the same call `rpc-server`'s `main()` makes at `b9789`) on a
  background thread. Bundles the compiled Metal `.metallib`. (Static linking needs
  `-all_load` so the Metal backend's self-registration isn't dead-stripped.)
- **A tiny Swift HTTP server** answering `GET /stats` with a `NodeStats`-shaped JSON
  payload on the airpcez port (`8675`).
- **A minimal status UI** — donation-budget slider, the LAN `ip:port` to copy into the
  cockpit, server on/off + **Restart** button, and live readouts (GB donated, active
  connections, `ProcessInfo.thermalState`).

The only thing the rest of the cluster knows about the iPad is its `/stats` JSON and its
RPC port. The host poller calls **only `GET /stats`** (confirmed in `poller.rs`), rewrites
the endpoint host to the reachable IP, and takes the port from the reported
`rpc_endpoint` — so that one endpoint plus the running RPC server is the entire contract.

## Components

### RPC bring-up (C shim + Swift)
Init Metal device → start the RPC server on `0.0.0.0:50052`, advertising the **donation
budget** as the backend's free/total memory. Runs on a background thread for the app's
foreground lifetime. **Restart** tears down and re-inits the backend (the desktop
"fresh worker restart" lesson, for leaked wired memory after an aborted load).

### Stats provider (Swift)
Mirrors the macOS unified-memory convention (real-free RAM separated from the Metal
working set):

- `name` — device name (e.g. "ipad-pro-m5")
- `role` — `"worker"`
- `ram_total_mib` — `ProcessInfo.physicalMemory`
- `ram_free_mib` — derived from `os_proc_available_memory()` (headroom before jetsam,
  post-entitlement)
- `devices` — exactly one `DeviceStats { name, kind: "metal", vram_total_mib = donation
  budget, vram_free_mib = budget − currently-wired Metal bytes, reliable: true }`
- `rpc_endpoint` — `"<self-ip>:50052"` (host rewrites the IP; only the port is load-bearing)
- `binary_version` — the pinned llama.cpp tag (e.g. `"b9789"`)
- `running` — RPC server thread is listening
- `sampled_at_unix` — current epoch seconds

### HTTP `/stats` server (Swift)
Serves the above JSON on `:8675`. That is the entire host-facing HTTP surface.

### Donation budget — governs placement via `/stats`
One configurable number sets the `/stats` device `vram_total_mib`, which the host planner
turns into `--tensor-split` — so the budget controls how many layers land on the iPad.
At `b9789` the RPC server has no free/total parameter (it self-reports the Metal device's
memory), so the budget cannot be capped at the RPC layer; we therefore report a
**conservative** budget in `/stats` so the planner under-fills against jetsam. Default ≈
**80% of startup `os_proc_available_memory()`**, leaving headroom for KV/compute/command
buffers and the OS. Exposed as a slider for empirical tuning (start conservative, push up).

## Data flow

1. **App launch** → read available memory, start the RPC server (`:50052`) and `/stats`
   server (`:8675`), both bound on the LAN.
2. **Add node** → type the iPad's `ip:8675` into the cockpit (manual add). The app
   displays that string for easy copying.
3. **Host polls `/stats`** → iPad appears as a node with one Metal device and its
   donatable VRAM; the existing `version_warnings` check runs.
4. **Plan** → the planner treats the iPad's Metal pool like any GPU and assigns it layers
   via `--tensor-split`.
5. **Launch** → host runs `llama-server --rpc …,<ipad-ip>:50052`; tensor ops for the
   iPad's layers are offloaded over RPC. Tokens stream from the host as usual.

## Memory & entitlements

- **Entitlements:** `com.apple.developer.kernel.increased-memory-limit` (raise the
  per-app ceiling toward physical RAM) and
  `com.apple.developer.kernel.extended-virtual-addressing` (large allocations). Both
  available with the paid account.
- **Why a budget, not "all of it":** Metal command/KV/compute buffers and the OS need
  headroom; over-advertising gets the app jetsam-killed mid-inference. Tune the slider up
  empirically.

## Error handling & edge cases

- **Backgrounding/suspension** — iOS suspends backgrounded apps → RPC dies. v1 keeps the
  app foreground with the idle timer disabled (keep-awake), device plugged in; the app
  warns if it loses foreground.
- **Thermals** — surface `ProcessInfo.thermalState`; warn on `serious`/`critical`
  (sustained RPC load will throttle).
- **Version mismatch** — the iPad reports `binary_version`; the host's existing warning
  fires if it ≠ host. Bumping llama.cpp means rebuilding the `xcframework` from the new tag.
- **Leaked memory after a failed/aborted load** — the **Restart server** button drops and
  re-inits the backend.
- **WiFi drop / poll timeout** — the host already greys out unreachable nodes and excludes
  them from `--rpc`; nothing iPad-specific needed.

## Build & distribution

- New top-level **`ios/AirpcezWorker/`** Xcode project (cannot be a Cargo crate).
- A **`scripts/build-llama-ios-xcframework.sh`** clones/builds llama.cpp at the **pinned
  tag** for `arm64` device (+ optional simulator) with Metal + RPC, producing the
  `xcframework`. The pin is what keeps the iPad's `binary_version` matched to the host.
- Signed with the team profile + the two entitlements; installed via Xcode (stable, no
  7-day expiry).
- A short doc note covers "bump llama.cpp on host and iPad together."

## Development environment

- **Test device:** M5 iPad Pro 13" running **iPadOS 27 Beta 2**.
- **Toolchain:** deploying to a 27 Beta 2 device requires an **Xcode whose SDK /
  device-support matches that beta** (typically the corresponding Xcode beta, or at least
  matching device-support files). This is the most likely "won't install" snag — listed
  in build prerequisites.
- **Beta memory behavior is a moving target:** jetsam ceilings, `increased-memory-limit`
  behavior, and thermal throttling can shift between beta builds — which is exactly why
  the donation budget is an **empirically-tuned slider**, not a hard-coded value.
  Re-validate the budget after OS updates.
- **API surface unaffected:** `os_proc_available_memory()`,
  `MTLDevice.recommendedMaxWorkingSetSize`, both entitlements, and the RPC/Metal backends
  are long-stable and present on 27.

## Testing strategy

- **Rust-side golden contract test:** capture a real iPad `/stats` payload as a fixture
  (`crates/airpcez/tests/fixtures/`) and assert it deserializes into `NodeStats` —
  catches Swift↔Rust wire drift without FFI.
- **On-device milestone validation** (the hard parts are inherently manual/hardware):
  - **M1 (de-risk first):** `xcframework` builds; RPC server + Metal starts on-device and
    accepts a connection from the host (proves the one real unknown).
  - **M2:** iPad shows up in the cockpit with correct stats; version check passes.
  - **M3:** host loads a model with layers placed on the iPad and generates tokens
    (capacity proven).
  - **M4:** entitlement + budget slider + lifecycle/thermal UX; push the budget up and
    confirm no jetsam kills.

## Milestones / phasing

1. **M1 — RPC+Metal on device (de-risk):** xcframework build script; C shim; start the
   RPC server in-process; verify the host can connect.
2. **M2 — Node in cockpit:** Swift `/stats` server + stats provider; manual add; version
   check; golden contract test on the Rust side.
3. **M3 — Real offload:** host places layers on the iPad and generates — capacity proven
   end to end.
4. **M4 — Memory + UX polish:** entitlements, donation-budget slider, keep-awake, thermal
   warnings, Restart button; empirically raise the budget.

## Out of scope / future

- ANE / Core ML / MLX path.
- Background or suspended serving.
- Auto-discovery; host-initiated remote worker restart.
- Multi-model; the iPad acting as host; auth.

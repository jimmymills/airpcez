# airpcez — Design Spec

**Date:** 2026-06-25
**Status:** Approved (brainstorming complete; ready for implementation planning)

## Overview

airpcez is a lightweight Rust control plane for a home **llama.cpp RPC cluster**. A single small binary runs on each machine and serves a local web UI from which you can:

- **register the machine as an RPC worker** (it runs `rpc-server`), or
- **run it as the host** (it runs `llama-server`, which exposes OpenAI-compatible endpoints + a built-in chat),

while showing **live RAM/VRAM stats for the whole cluster** and **suggesting safe `-ngl` / `--tensor-split` settings with a pre-flight fit check** so launches stop failing with the OOM trial-and-error this project was born from.

**Ethos:** "exo without the component bloat." airpcez is a *control plane*, not an inference engine — it configures, launches, and monitors the official llama.cpp binaries we already proved work (release `b9789`). The Rust app stays tiny; llama.cpp does the heavy lifting.

## Goals

- One web UI to stand up either cluster role and tweak settings without fighting the CLI.
- A single aggregated **cockpit** showing every node's live RAM/VRAM/CPU/role/status.
- **Smart assist:** auto-suggest `-ngl`/`--tensor-split`, flag broken devices, and run a "will this fit?" pre-flight check; on failure, propose the fix.
- Low overhead — a thin supervisor wrapping proven binaries, not a multi-component stack.
- A **portable Rust core** so an iPad *cockpit* (browser) works for free and an iPad *compute-node* app stays possible later.

## Non-goals (v1)

Reverse-proxy for one stable OpenAI URL; iPad/native compute-node app; auto-discovery (manual node add is deliberate, to stay light); Windows / AMD-ROCm / Intel-Mac stats backends; binary auto-download/management; multi-model hosting / model switching; auth (LAN-trusted, single user).

## Key decisions (from brainstorming)

| Decision | Choice |
|---|---|
| Interface | Local **web UI** (browser), reachable across the LAN |
| iPad | **Cockpit-first** (Safari → host UI); portable core; embedded-compute deferred |
| Stats | **One aggregated cluster dashboard**; host polls each node's `/stats`; the node list doubles as the `--rpc` list |
| Assist level | **Smart** — suggest `-ngl`/`tensor-split` + pre-flight fit check |
| Architecture | **Approach A** — thin Rust supervisor wrapping `rpc-server`/`llama-server`; reuse `b9789` binaries |
| Dashboard layout | **A** — node list pinned left, config → fit → launch → logs workspace on the right |

## Architecture

One `airpcez` binary per machine; the same binary is worker or host (role is config). Layered so the portable logic is isolated from OS-specific code:

### `airpcez-core` — platform-agnostic library (the part that ports to iPad)
- **Cluster model** — `Node { id, name, addr, role, capabilities }`, `ClusterConfig { nodes, model, runtime settings }`. One node list serves as both the `--rpc` list and the stats-poll list.
- **Flag builder** — pure functions: `(config, per-node caps) → exact argv` for `rpc-server` / `llama-server` (`-ngl`, `--tensor-split`, `--device`, `--rpc`, `--cpu-moe`/`--n-cpu-moe`, `-c`, `-hf`/`-m`, `--host`/`--port`, `-d`). Pure → fully unit-testable. Encodes the flag knowledge from this project once.
- **Planner / suggestion engine** — pure: `(live stats + model metadata) → suggested -ngl/tensor-split + fit verdict`; flags unreliable devices. The differentiator.
- **Stats trait** — `StatsProvider → NodeStats { ram_total/free, vram-per-device (with reliability flag), cpu, role, running, binary_version }`.
- **Process trait** — `ProcessBackend { spawn, stop, status }` so spawning is swappable (desktop spawns; an iPad build could swap it).
- **OpenAI client/proxy** — thin client to the local `llama-server` (reverse-proxy is a later option; v1 surfaces the URL).

### Platform layer — native-only impls
- **Stats backends:** macOS (`sysctl`/`vm_stat` + Metal working-set, **correctly separating real-free RAM from Metal's max working set** — a lesson from this project), Linux (`sysinfo` + NVML/`nvidia-smi`, **detecting the bogus/overflow VRAM report** seen on the Vulkan/2080 path). Windows later.
- **Process supervisor:** `tokio` child processes — spawn/monitor/restart `rpc-server`/`llama-server`, capture and stream their logs.

### App/server layer — the binary
- **`axum` HTTP + WebSocket server** — serves the embedded web UI, a JSON API (config CRUD, launch/stop, node list), and a WS live-stats stream.
- **Host vs worker:** worker exposes `/stats` + runs `rpc-server`; host *also* polls nodes, aggregates, builds + launches `llama-server`, and links to its OpenAI endpoint + built-in chat (chat is not rebuilt — `llama-server` ships one).
- **Config persistence:** a small TOML file (node list + last-used settings).

### Web UI
Lightweight embedded HTML/JS (no heavy SPA framework, honoring the low-overhead ethos), **layout A**. Works in any browser including iPad Safari → that's the free cockpit.

The only platform-specific code is the stats + process backends; everything else is the portable core.

## Data flow

1. **Worker comes up** — `airpcez` auto-detects local stats and exposes its `rpc-server` (llama.cpp RPC port, e.g. `:50052`, pinned to the good device via `-d`) plus its own `/stats` (airpcez port, e.g. `:8675`). Headless boxes are reached from any browser.
2. **Host configures** — add each worker by airpcez address; host polls every `/stats` → live cockpit. `/stats` reports each worker's `rpc-server` endpoint, so the host's `--rpc` list is *derived*, not typed. Pick a model (`-hf repo:quant` or local GGUF) → airpcez reads its metadata (size, layer count, MoE?).
3. **Launch** — flag-builder assembles the `llama-server` argv from config + live caps; **pre-flight** validates fit against current stats and optionally restarts workers fresh first; supervisor spawns it, streams logs, parses for "loaded" vs OOM signatures. Success → show OpenAI base URL + chat link. OOM → capture + propose the fix.
4. **Live stats** — each node samples on an interval; host aggregates; all open UIs update over WebSocket.
5. **Serving** — clients hit the host's `llama-server` OpenAI endpoint directly (stable-URL reverse-proxy deferred).

## Suggestion / pre-flight engine (pure, unit-tested)

Given per-node live stats + model metadata, it:
1. **Filters devices** — drops anything flagged unreliable (VRAM value that overflows or exceeds physical → the Vulkan case) or below threshold, with a visible note.
2. **Places layers** — fills reliable GPUs up to `(real free VRAM − headroom)`; sets `--tensor-split` ∝ *real* free VRAM; parks the output tensor on the roomiest device (`--main-gpu`); spills remainder to CPU. For MoE, switches to the `--cpu-moe` / `--n-cpu-moe N` sweet-spot strategy.
3. **Verdicts the fit** — sums weights + KV(context) + compute buffers + per-device overhead vs availability; returns ✅ fits / ⚠️ tight / ❌ won't-fit **and names the bottleneck device**.
4. **Encodes operational lessons** — recommend/auto-do a fresh worker restart before launch (aborted loads leak wired memory); require matching llama.cpp versions across nodes.

## Error handling & edge cases

- **Launch/OOM failure** — parse stderr for known signatures (`ErrorOutOfDeviceMemory`, `unable to allocate … buffer`, `failed to load model`); show raw error + engine's proposed fix (exclude device / lower `-ngl` / smaller quant) with one-click apply-and-relaunch.
- **Version mismatch** — host warns/blocks if a worker's `rpc-server` version ≠ the host's `llama-server` version (the #1 silent RPC failure).
- **Leaked memory after failed load** — pre-flight offers/auto-does a fresh worker restart.
- **Unreachable / dropped node** — poll timeout → greyed "offline (last seen…)", excluded from `--rpc`; dashboard keeps working; stale samples badged.
- **Process crash vs clean stop** — distinguished, surfaced with last log lines + restart button.
- **Missing binary / port in use** — flagged in the UI before launch.

## Testing strategy

The hard logic is **pure**, so most is unit-tested with no hardware:
- **Flag-builder** — golden tests: `(config, caps) → exact argv`.
- **Suggestion/pre-flight engine** — fixtures including **this project's 70B scenario as a regression test** (Vulkan bogus-VRAM + M2 leak masking real-free): assert it excludes `Vulkan0`, sizes the split to real free VRAM, and verdicts correctly.
- **Stats parsers** — captured real `nvidia-smi` / `vm_stat` / `sysctl` outputs as fixtures, incl. overflow-VRAM detection.
- **Process supervisor** — a `ProcessBackend` fake simulating spawn/exit/crash + canned OOM log streams, testing the failure→suggestion path with no real binaries.
- **HTTP/WS API** — integration tests over the running server.
- **End-to-end** — manual, against the real cluster (the bulk is unit-testable because the hard logic is pure).

## Roles, ports, config

- **Worker:** `rpc-server -H 0.0.0.0 -p <rpc_port> -d <good device>` + airpcez UI/`/stats` on `<ui_port>`.
- **Host:** polls nodes, aggregates, builds + launches `llama-server` on `<llama_port>`, links OpenAI/chat.
- **Node entry:** airpcez address; `/stats` reports rpc endpoint + binary version + capabilities.
- **Default ports (configurable):** airpcez UI `8675`, llama.cpp RPC `50052`, `llama-server` `8080`.
- **Binaries:** v1 assumes the `b9789` `rpc-server`/`llama-server` exist at a configured path; UI flags "found / not found" (auto-download deferred).

## Scope — v1 vs later

**In v1:** web UI (layout A); worker + host roles; manual node list; aggregated live stats (RAM/VRAM/CPU); model pick (`-hf` or local GGUF); flag-builder; suggestion + pre-flight engine; launch/stop/restart + logs; surface OpenAI URL + chat link; TOML config; version-match warning; **macOS + Linux/NVIDIA** stats backends.

**Deliberately later:** stable-URL reverse-proxy; iPad/native compute-node app; auto-discovery; Windows / AMD-ROCm / Intel-Mac backends; binary auto-download; multi-model hosting; auth.

## Open questions / future

- Reverse-proxy for one stable OpenAI URL (and model routing).
- iPad compute-node app: Rust core + Swift/WKWebView shell linking `libllama` with Metal.
- Discovery, Windows/AMD backends, a binary download manager.

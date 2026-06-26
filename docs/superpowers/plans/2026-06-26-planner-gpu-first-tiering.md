# Planner GPU-first Tiering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `suggest_plan` place models by a strict capacity hierarchy — GPUs → host CPU → CPU-only node(s) over RPC — so "Suggest settings" fills GPUs and host RAM before touching a CPU-only node, and never recommends a GPU-less node as host.

**Architecture:** Refactor the pure function `suggest_plan` (in `crates/airpcez-core/src/planner.rs`) to compute three capacity tiers, verdict against GPU+host-CPU first, and only engage CPU-only nodes on overflow. Add a worker-config coupling so a CPU-only node serves its RAM-backed CPU device over RPC. The actual RPC-CPU layer routing (tier 3) is gated behind an empirical verification.

**Tech Stack:** Rust (workspace crates `airpcez-core`, `airpcez`), `cargo test`, axum cockpit. No new dependencies.

## Global Constraints

- No new crate dependencies.
- Preserve existing GPU-packing heuristics and safety margins verbatim: dense `ngl = gpu_pool * 10/11 / per_layer`; MoE `gpu_need = total + total/10`; `GPU_HEADROOM_MIB = 1024`; `CPU_HEADROOM_MIB = 2048`. These fix llama.cpp "failed to fit params" — do not change them.
- Unreliable GPU devices (`reliable == false`, e.g. the bogus-Vulkan case) stay excluded from the GPU pool and keep their `exclude_notes` entry.
- `advertise_gpu` defaults to `true` (M1/M2/CUDA nodes unchanged); only CPU-only workers set it `false`.
- New `Plan` fields use `#[serde(default)]` so the cockpit JSON stays backward-compatible.
- The worker `rpc-server` and host must stay on matching llama.cpp build (currently `b9800`).
- Run from the repo root `/Users/jimmy.mills/Developer/airpcez`. Worker box: `ssh jimmy@192.168.0.111`; host: `192.168.0.24`.

---

## File Structure

- `crates/airpcez/src/config.rs` — `rpc_device_filter()` returns `"CPU"` when `advertise_gpu == false` (worker serves its RAM-backed CPU device). Unit-tested in-file.
- `crates/airpcez-core/src/planner.rs` — `suggest_plan` rewrite (tiers, verdict, `host_hint`); two new `Plan` fields. Unit-tested in-file.
- (Tier-3, contingent on verification) `crates/airpcez-core/src/planner.rs` + `crates/airpcez/src/server.rs` (`host_launch`/`build_launch_spec`) — emit and honor an RPC-CPU tensor-split share.
- Deploy artifacts (no source): `intel-mbp` `/Users/jimmy/airpcez/airpcez.toml` + rebuilt `airpcez` binary.

---

## Task 1: Worker serves its CPU device when GPU-less

**Files:**
- Modify: `crates/airpcez/src/config.rs` (`rpc_device_filter`, ~line 53)
- Test: `crates/airpcez/src/config.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: existing `Config { advertise_gpu: bool, rpc_device: Option<String> }`.
- Produces: `Config::rpc_device_filter(&self) -> Option<String>` now returns `Some("CPU")` when `advertise_gpu == false` and no explicit `rpc_device` is set.

- [ ] **Step 1: Write the failing test**

Add to `crates/airpcez/src/config.rs` tests module:

```rust
#[test]
fn rpc_device_filter_cpu_only_worker_serves_cpu() {
    // A CPU-only worker (advertise_gpu=false) must serve its RAM-backed CPU device,
    // not the default 0-MiB BLAS accelerator the rpc-server would otherwise pick.
    let mut c = Config::default();
    c.advertise_gpu = false;
    assert_eq!(c.rpc_device_filter(), Some("CPU".to_string()));

    // Explicit rpc_device still wins over the advertise_gpu default.
    c.rpc_device = Some("CUDA0".into());
    assert_eq!(c.rpc_device_filter(), Some("CUDA0".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez --lib rpc_device_filter_cpu_only_worker_serves_cpu`
Expected: FAIL — returns `None` (or platform default), not `Some("CPU")`.

- [ ] **Step 3: Implement**

In `crates/airpcez/src/config.rs`, edit `rpc_device_filter` so the `advertise_gpu == false` case is handled before the platform default:

```rust
pub fn rpc_device_filter(&self) -> Option<String> {
    if let Some(d) = &self.rpc_device {
        return Some(d.clone());
    }
    // A CPU-only worker advertises no GPU; serve its RAM-backed CPU device over RPC
    // (rpc-server otherwise prefers a 0-MiB BLAS accelerator the host can't offload to).
    if !self.advertise_gpu {
        return Some("CPU".to_string());
    }
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("MTL0".to_string())
    } else {
        None
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p airpcez --lib config::`
Expected: PASS (the new test plus the existing `rpc_device_filter_*` tests).

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/config.rs
git commit -m "feat(config): CPU-only worker serves -d CPU so the host can offload to it"
```

---

## Task 2: Tiered placement in `suggest_plan`

Rewrite `suggest_plan` to compute GPU / host-CPU / remote-CPU tiers, verdict against GPU+host-CPU first, and a host-aware `host_hint`. Add two `Plan` fields. Update the existing tests whose semantics changed, and add tests for the new behavior. The GPU-packing math (ngl/cpu_moe) is copied verbatim.

**Files:**
- Modify: `crates/airpcez-core/src/planner.rs` — `Plan` struct (~line 26) and `suggest_plan` (~line 70)
- Test: `crates/airpcez-core/src/planner.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ClusterStatus { nodes: Vec<NodeSnapshot> }`; `NodeSnapshot { entry, stats: Option<NodeStats>, reachable }`; `NodeStats { name, role: Role, ram_free_mib, devices: Vec<DeviceStats>, rpc_endpoint: Option<String> }`; `DeviceStats { name, kind: DeviceKind, vram_free_mib, vram_total_mib, reliable }`; `Role::{Host, Worker}`; `DeviceKind::{Metal, Cuda}`.
- Produces: `Plan` gains `remote_cpu_pool_mib: u64` and `rpc_cpu_nodes: Vec<String>`; `cpu_pool_mib` now means **host** CPU (tier 2); `suggest_plan(&ClusterStatus, &ModelMeta, u32) -> Plan` signature unchanged.

- [ ] **Step 1: Add the new `Plan` fields**

In `crates/airpcez-core/src/planner.rs`, inside `struct Plan`, after `pub host_hint: Option<String>,` add:

```rust
    /// Tier-3 capacity: reclaimable RAM (MiB) of CPU-only worker nodes, usable via RPC.
    /// 0 when there are none.
    #[serde(default)]
    pub remote_cpu_pool_mib: u64,
    /// rpc-server endpoints of CPU-only nodes to engage as last-resort CPU spillover.
    /// Empty unless the model overflows GPU + host CPU (tier 3 engaged).
    #[serde(default)]
    pub rpc_cpu_nodes: Vec<String>,
```

- [ ] **Step 2: Write the failing tests**

Add a `host(...)` helper and four tests to the planner tests module. (The existing `node(...)` helper stays as a Worker-role node.)

```rust
// A Host-role node (the one that would run llama-server).
fn host(name: &str, ram_free: u64, devices: Vec<DeviceStats>) -> NodeSnapshot {
    let mut s = node(name, ram_free, devices);
    if let Some(st) = s.stats.as_mut() { st.role = Role::Host; }
    s
}
// A reachable CPU-only worker with an rpc endpoint and no GPU devices.
fn cpu_worker(name: &str, ram_free: u64, ep: &str) -> NodeSnapshot {
    let mut s = node(name, ram_free, vec![]);
    if let Some(st) = s.stats.as_mut() { st.rpc_endpoint = Some(ep.into()); }
    s
}

#[test]
fn host_hint_prefers_gpu_host_never_cpu_only_node() {
    // GPU host with 32 GB + a CPU-only 32 GB node. host_hint must point at the GPU host.
    let cluster = ClusterStatus { nodes: vec![
        host("gpu-host", 30000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 7500, true)]),
        cpu_worker("cpu-box", 31000, "192.168.0.111:50052"),
    ], warnings: vec![] };
    let meta = ModelMeta { total_mib: 6000, n_layers: 32, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);
    let hint = p.host_hint.expect("host hint present");
    assert!(hint.contains("gpu-host"), "must recommend the GPU host: {hint}");
    assert!(!hint.contains("cpu-box"), "must never recommend the CPU-only node: {hint}");
}

#[test]
fn cpu_only_node_excluded_from_primary_pool() {
    // 6 GB model fits on the GPU host alone; the CPU-only node's RAM must NOT inflate
    // cpu_pool_mib (tier 2 = host CPU only) and must NOT be wired (no overflow).
    let cluster = ClusterStatus { nodes: vec![
        host("gpu-host", 20000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
        cpu_worker("cpu-box", 31000, "192.168.0.111:50052"),
    ], warnings: vec![] };
    let meta = ModelMeta { total_mib: 6000, n_layers: 32, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);
    assert_eq!(p.fit.fit, Fit::Fits);
    assert_eq!(p.cpu_pool_mib, 20000 - 2048, "cpu_pool_mib is the host's CPU only");
    assert_eq!(p.remote_cpu_pool_mib, 31000 - 2048, "CPU node tracked as a separate tier");
    assert!(p.rpc_cpu_nodes.is_empty(), "CPU node not engaged when model fits GPU+host");
}

#[test]
fn tier3_engages_only_on_overflow_and_wires_cpu_node() {
    // Model bigger than GPU(7 GB) + host CPU(8 GB) ≈ 15 GB but < +CPU node(31 GB):
    // tier 3 engages and the CPU node's endpoint is wired.
    let cluster = ClusterStatus { nodes: vec![
        host("gpu-host", 10000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
        cpu_worker("cpu-box", 33000, "192.168.0.111:50052"),
    ], warnings: vec![] };
    let meta = ModelMeta { total_mib: 20000, n_layers: 40, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);
    assert_eq!(p.fit.fit, Fit::Tight);
    assert!(p.fit.detail.contains("RPC"), "verdict names the RPC CPU spillover: {}", p.fit.detail);
    assert_eq!(p.rpc_cpu_nodes, vec!["192.168.0.111:50052".to_string()]);
}

#[test]
fn wont_fit_beyond_all_three_tiers() {
    let cluster = ClusterStatus { nodes: vec![
        host("gpu-host", 10000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
        cpu_worker("cpu-box", 12000, "192.168.0.111:50052"),
    ], warnings: vec![] };
    let meta = ModelMeta { total_mib: 90000, n_layers: 80, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);
    assert_eq!(p.fit.fit, Fit::WontFit);
    assert!(p.rpc_cpu_nodes.is_empty(), "no wiring when even CPU nodes can't make it fit");
}
```

- [ ] **Step 3: Run tests to verify they fail to compile/pass**

Run: `cargo test -p airpcez-core --lib planner::`
Expected: FAIL — `Plan` missing fields / `host`/`cpu_worker` undefined until Step 1 + this step land, then assertion failures until Step 4.

- [ ] **Step 4: Rewrite `suggest_plan`**

Replace the body of `suggest_plan` in `crates/airpcez-core/src/planner.rs` with:

```rust
pub fn suggest_plan(cluster: &ClusterStatus, meta: &ModelMeta, ctx: u32) -> Plan {
    use crate::model::{DeviceKind, Role};

    // The node that will run llama-server: first reachable Host-role node, else first reachable.
    let host_idx = cluster.nodes.iter().position(|n| n.reachable
            && n.stats.as_ref().is_some_and(|s| s.role == Role::Host))
        .or_else(|| cluster.nodes.iter().position(|n| n.reachable));

    // --- Tier 1: GPU pool across ALL reachable nodes (unchanged) ---
    let mut gpu_pool = 0u64;
    let mut splits: Vec<u64> = Vec::new();
    let mut exclude_notes = Vec::new();
    let mut roomiest: Option<(String, u64)> = None;
    for n in &cluster.nodes {
        let Some(st) = &n.stats else { continue };
        if !n.reachable { continue; }
        for d in &st.devices {
            if d.reliable && d.vram_total_mib > 0 {
                let usable = d.vram_free_mib.saturating_sub(GPU_HEADROOM_MIB);
                gpu_pool += usable;
                splits.push(usable);
                if roomiest.as_ref().is_none_or(|(_, best)| usable > *best) {
                    roomiest = Some((format!("{}/{}", n.entry.name, d.name), usable));
                }
            } else if !d.reliable {
                exclude_notes.push(format!(
                    "{}/{}: unreliable VRAM reading ({} MiB) — exclude this device",
                    n.entry.name, d.name, d.vram_free_mib));
            }
        }
    }

    // --- Tier 2: host CPU (host's reclaimable RAM minus its own Metal carve-out) ---
    let host_stats = host_idx.and_then(|i| cluster.nodes[i].stats.as_ref());
    let host_cpu = host_stats.map(|s| {
        let host_unified: u64 = s.devices.iter()
            .filter(|d| d.reliable && matches!(d.kind, DeviceKind::Metal))
            .map(|d| d.vram_free_mib).sum();
        s.ram_free_mib.saturating_sub(CPU_HEADROOM_MIB + host_unified)
    }).unwrap_or(0);

    // --- Tier 3: CPU-only nodes (reachable, not the host, no reliable GPU) ---
    let mut remote_cpu = 0u64;
    let mut cpu_node_eps: Vec<String> = Vec::new();
    for (i, n) in cluster.nodes.iter().enumerate() {
        if !n.reachable || Some(i) == host_idx { continue; }
        let Some(st) = &n.stats else { continue };
        if st.devices.iter().any(|d| d.reliable && d.vram_total_mib > 0) { continue; }
        remote_cpu += st.ram_free_mib.saturating_sub(CPU_HEADROOM_MIB);
        if let Some(ep) = &st.rpc_endpoint { cpu_node_eps.push(ep.clone()); }
    }

    // --- Layer packing (Tiers 1-2): GPU-first logic, unchanged ---
    let per_layer = (meta.total_mib / meta.n_layers.max(1) as u64).max(1);
    let tensor_split = ratio_string(&splits);
    let (ngl, cpu_moe, no_mmap) = if meta.is_moe {
        let gpu_need = meta.total_mib + meta.total_mib / 10;
        let shortfall = gpu_need.saturating_sub(gpu_pool);
        if shortfall == 0 {
            (meta.n_layers, Some("off".to_string()), false)
        } else {
            let n = shortfall.div_ceil(per_layer).min(meta.n_layers as u64) as u32;
            let s = if n >= meta.n_layers { "all".to_string() } else { n.to_string() };
            (meta.n_layers, Some(s), true)
        }
    } else {
        (((gpu_pool * 10 / 11 / per_layer) as u32).min(meta.n_layers), None, false)
    };

    // --- Fit verdict: GPU + host CPU first; CPU nodes only on overflow ---
    let required = meta.total_mib + kv_mib(meta.n_layers, ctx);
    let primary = gpu_pool + host_cpu;
    let roomiest_suffix = match &roomiest {
        Some((name, mib)) => format!(" — roomiest GPU {name} {mib} MiB"),
        None => " — no reliable GPU detected".to_string(),
    };
    let mut rpc_cpu_nodes: Vec<String> = Vec::new();
    let (fit, detail) = if required + required / 10 <= primary {
        (Fit::Fits, format!("fits — ~{} MiB headroom on {} MiB GPU + {} MiB host CPU{}",
            primary.saturating_sub(required), gpu_pool, host_cpu, roomiest_suffix))
    } else if required <= primary {
        (Fit::Tight, format!("tight — needs {} MiB; GPU + host CPU is {} MiB ({} GPU + {} host CPU){}",
            required, primary, gpu_pool, host_cpu, roomiest_suffix))
    } else if required <= primary + remote_cpu {
        rpc_cpu_nodes = cpu_node_eps;
        (Fit::Tight, format!("tight — +{} MiB spills to CPU node(s) via RPC (slow last resort); {} GPU + {} host CPU + {} remote CPU{}",
            required - primary, gpu_pool, host_cpu, remote_cpu, roomiest_suffix))
    } else {
        (Fit::WontFit, format!("won't fit — needs {} MiB but GPU + host + CPU nodes is only {} MiB; use a smaller quant{}",
            required, primary + remote_cpu, roomiest_suffix))
    };

    // --- host_hint: confirm the GPU host; warn if the host has no GPU ---
    let host_hint = host_stats.map(|s| {
        let host_has_gpu = s.devices.iter().any(|d| d.reliable && d.vram_total_mib > 0);
        if host_has_gpu {
            format!("run llama-server on '{}' (the host) — {} MiB free RAM + local GPU", s.name, s.ram_free_mib)
        } else {
            let best = roomiest.as_ref().map(|(n, _)| n.as_str()).unwrap_or("a GPU node");
            format!("host '{}' has no GPU — run llama-server on a GPU node ({}) for GPU acceleration", s.name, best)
        }
    });

    Plan { ngl, tensor_split, cpu_moe, exclude_notes,
        fit: FitVerdict { fit, detail }, gpu_pool_mib: gpu_pool, cpu_pool_mib: host_cpu,
        remote_cpu_pool_mib: remote_cpu, rpc_cpu_nodes,
        warnings: Vec::new(),
        flash_attn: Some("on".to_string()), no_mmap, host_hint }
}
```

- [ ] **Step 5: Update the two existing tests whose semantics changed**

In `plan_json_roundtrips`, add the two new fields to the `Plan { .. }` literal:

```rust
            remote_cpu_pool_mib: 12000, rpc_cpu_nodes: vec!["10.0.0.9:50052".into()],
```

In `recommends_flash_attn_and_roomiest_host`, make `big-box` the Host node so `host_hint` points at it (host is now role-based, not max-RAM):

```rust
    let cluster = ClusterStatus {
        nodes: vec![
            node("small-mac", 16000, vec![gpu("MTL0", DeviceKind::Metal, 12000, 10000, true)]),
            host("big-box",   64000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
        ],
        warnings: vec![],
    };
```

(The assertion `hint.contains("big-box")` then holds: big-box is the GPU host.)

- [ ] **Step 6: Run the full planner + core test suite**

Run: `cargo test -p airpcez-core`
Expected: PASS — the four new tests, both updated tests, and all unchanged planner tests (`plan_fits_small_model_on_one_gpu`, `plan_excludes_bogus_vram_and_sizes_to_real_free_70b`, the four MoE/dense margin tests, `kv_*`, `ratio_*`).

- [ ] **Step 7: Run the dependent crate tests**

Run: `cargo test -p airpcez`
Expected: PASS — `suggest_endpoint` and `host_launch` integration tests still pass (new `Plan` fields are `#[serde(default)]`).

- [ ] **Step 8: Commit**

```bash
git add crates/airpcez-core/src/planner.rs
git commit -m "feat(planner): tiered GPU -> host-CPU -> remote-CPU placement; fix host_hint and CPU over-count"
```

---

## Task 3: Rebuild and redeploy the worker

Deliver the Task 1 behavior to `intel-mbp` so it serves its 32 GB CPU device, and confirm the cluster still reports it cleanly.

**Files:** none (build + deploy).

- [ ] **Step 1: Cross-compile the x86_64 binary**

Run: `cargo build --release --target x86_64-apple-darwin`
Expected: `Finished release`. Verify: `file target/x86_64-apple-darwin/release/airpcez` → `Mach-O 64-bit executable x86_64`.

- [ ] **Step 2: Ship the binary**

Run: `scp -q target/x86_64-apple-darwin/release/airpcez jimmy@192.168.0.111:/Users/jimmy/airpcez/airpcez`

(`advertise_gpu = false` is already in the worker's `airpcez.toml`, so `rpc_device_filter()` now yields `-d CPU` automatically. No toml edit needed.)

- [ ] **Step 3: Restart the worker cleanly (kill orphaned rpc-server too)**

Run:
```bash
ssh jimmy@192.168.0.111 'cd /Users/jimmy/airpcez && chmod +x airpcez; pkill -x airpcez; pkill -f "llama-b9800-x64/rpc-server"; sleep 2; nohup ./airpcez --worker >/tmp/airpcez.log 2>&1 </dev/null & disown; sleep 3; head -2 /tmp/airpcez.log'
```
Expected: log shows `started rpc-server ... on 0.0.0.0:50052`.

- [ ] **Step 4: Verify rpc-server now serves the 32 GB CPU device**

Run: `ssh jimmy@192.168.0.111 'grep -A3 Devices /tmp/airpcez.log || tail -20 /tmp/airpcez.log'`
Expected: the served device is `CPU: Intel(R) Core ... (32768 MiB, 32768 MiB free)`, NOT `BLAS: Accelerate (0 MiB ...)`. (If the banner isn't in the log, restart with a 2s foreground `./llama-b9800-x64/rpc-server -H 127.0.0.1 -p 50098 -d CPU` and read its banner.)

- [ ] **Step 5: Verify the cluster view is clean**

Run: `curl -s --max-time 8 http://192.168.0.24:8675/cluster | python3 -m json.tool | grep -E 'intel-mbp|binary_version|reachable' | head`
Expected: `intel-mbp` reachable, `b9800`, no version warning.

- [ ] **Step 6: Commit (deploy note only — no source change)**

No commit needed; record completion in the task tracker.

---

## Task 4: Verification gate — can llama.cpp offload to a remote CPU device?

This decides whether Task 5 (tier-3 auto-routing) is implemented as real RPC-CPU routing or left as the already-shipped "report it" verdict. **Do not start Task 5 until this passes.**

**Files:** none (empirical experiment). Use a small model to keep it fast.

- [ ] **Step 1: Pick a small GGUF already cached on the host**

Run: `ssh jimmy@192.168.0.24 'ls /mnt/ssd/llama/models | head'`
Expected: a small model directory (e.g. a 1–4B Q4). Note its `-hf` repo or local path. If none is small, download a ~1B Q4 first.

- [ ] **Step 2: Launch llama-server on the host, forcing layers onto the remote CPU RPC device**

From the host `.24`, run llama-server with `--rpc 192.168.0.111:50052`, a `--device` list that includes the RPC slot, and a `--tensor-split` that gives the RPC CPU slot a non-zero share. Example shape (adjust device names to what llama.cpp prints):
```bash
ssh jimmy@192.168.0.24 '/home/jimmy/llama.cpp/build/bin/llama-server \
  -hf <small-model> --rpc 192.168.0.111:50052 \
  -ngl 99 --tensor-split 0,1 --device RPC0 \
  -c 2048 --host 127.0.0.1 --port 9099 & sleep 60; \
  curl -s http://127.0.0.1:9099/v1/models; kill %1'
```

- [ ] **Step 3: Observe where layers landed**

Read the llama-server startup log lines that report per-device buffer sizes / "assigned to device". 
Expected (PASS): a non-zero buffer is allocated on the RPC (`intel-mbp`) device and a token decodes (the `/v1/models` call succeeds and a `/v1/completions` returns text).
FAIL: the RPC device shows 0 MiB allocated, or llama-server errors / refuses to place layers on the CPU-backed RPC device.

- [ ] **Step 4: Record the device-string format**

Write down, in the plan tracker, the exact `--device` token llama.cpp used for the RPC CPU slot (e.g. `RPC0`, `RPC[192.168.0.111:50052]`) and the device ordering (local devices first, then RPC in `--rpc` order). Task 5 needs this verbatim.

- [ ] **Step 5: Decision**

- PASS → proceed to Task 5 (auto-route tier-3).
- FAIL → STOP. The shipped Task 2 verdict already reports tier-3 as a "report it" suggestion (`rpc_cpu_nodes` + detail string); that is the final behavior. Note the failure mode in the spec's "Risks" section and close out.

---

## Task 5 (contingent on Task 4 PASS): Auto-route tier-3 overflow over RPC

Only the device-string format from Task 4 Step 4 is unknown; the allocation math is specified here. When tier 3 is engaged, the planner computes a remote-CPU tensor-split share and the launch wires the CPU node's device into `--device`/`--tensor-split`.

**Files:**
- Modify: `crates/airpcez-core/src/planner.rs` — when `!rpc_cpu_nodes.is_empty()`, extend `tensor_split` with a remote-CPU share and raise `ngl` to cover the overflow layers.
- Modify: `crates/airpcez/src/server.rs` (`build_launch_spec` ~line 212 / `host_launch` ~line 417) and/or `crates/airpcez-core/src/flags.rs` — order the CPU node(s) last in `--rpc` and append their device token (from Task 4) to `--device`, matching the tensor-split positions.
- Test: `crates/airpcez-core/src/planner.rs` and `crates/airpcez/src/server.rs` test modules.

**Interfaces:**
- Consumes: `Plan { rpc_cpu_nodes, tensor_split, ngl, remote_cpu_pool_mib }` from Task 2.
- Produces: a launch spec whose `--tensor-split` has one trailing entry per engaged CPU node and whose `--device` lists those nodes' RPC device tokens last.

- [ ] **Step 1: Write the failing planner test**

```rust
#[test]
fn tier3_adds_remote_cpu_share_to_tensor_split() {
    let cluster = ClusterStatus { nodes: vec![
        host("gpu-host", 10000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
        cpu_worker("cpu-box", 33000, "192.168.0.111:50052"),
    ], warnings: vec![] };
    let meta = ModelMeta { total_mib: 20000, n_layers: 40, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);
    // GPU(1 entry) + one remote-CPU entry → tensor_split has 2 comma fields.
    assert_eq!(p.tensor_split.as_deref().unwrap().split(',').count(), 2);
    // ngl now covers GPU layers + the remote-CPU overflow layers (more than the GPU-only count).
    assert!(p.ngl > (8000u64 * 10 / 11 / (20000/40)) as u32);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p airpcez-core --lib tier3_adds_remote_cpu_share_to_tensor_split`
Expected: FAIL — `tensor_split` has only the GPU entry.

- [ ] **Step 3: Implement the planner side**

In `suggest_plan`, in the tier-3 branch (where `rpc_cpu_nodes = cpu_node_eps;`), after computing `required`/`primary`: size the remote share as the overflow `overflow = required.saturating_sub(primary)`, push `overflow` (per CPU node, split evenly) onto a copy of `splits`, recompute `tensor_split = ratio_string(&splits_with_remote)`, and set `ngl` to cover the GPU-fitting layers plus `overflow.div_ceil(per_layer)` overflow layers (capped at `n_layers`). Keep host-CPU layers as `n_layers - ngl`. Use the existing `ratio_string` helper. (Exact ordering: GPU entries first, then remote-CPU entries, matching the `--device` order from Step 5.)

- [ ] **Step 4: Run the planner test to verify it passes**

Run: `cargo test -p airpcez-core --lib planner::`
Expected: PASS (new test + all prior).

- [ ] **Step 5: Wire the launch (use the device token from Task 4 Step 4)**

In `host_launch`, order CPU-only node endpoints LAST in `eps`, and pass a `--device` string that lists local + GPU-RPC devices first, then the CPU nodes' RPC device token(s) (the verbatim format recorded in Task 4). Add a `build_launch_spec` test asserting the argv contains `--rpc ...192.168.0.111:50052` last and a `--device` ending in the CPU RPC token. (Concrete argv strings filled from Task 4's recorded format.)

- [ ] **Step 6: Run the server tests**

Run: `cargo test -p airpcez`
Expected: PASS.

- [ ] **Step 7: End-to-end smoke (oversized model)**

Launch via the cockpit/`/host/launch` a model larger than GPU+host CPU and confirm `intel-mbp` receives a non-zero buffer and a token decodes. (Reuse the Task 4 harness with a real over-52 GB model or a smaller GPU/host cap.)

- [ ] **Step 8: Commit**

```bash
git add crates/airpcez-core/src/planner.rs crates/airpcez/src/server.rs
git commit -m "feat(planner): auto-route tier-3 overflow to CPU node(s) over RPC"
```

---

## Self-Review

**Spec coverage:**
- Tier model (GPU/host-CPU/remote-CPU) → Task 2. ✓
- host_hint never picks CPU-only node → Task 2 (`host_hint_prefers_gpu_host_never_cpu_only_node`). ✓
- cpu_pool accounting fix → Task 2 (`cpu_only_node_excluded_from_primary_pool`). ✓
- Worker reconfig (`-d CPU` via `advertise_gpu=false`) → Task 1 + Task 3. ✓
- Verification gate → Task 4. ✓
- Tier-3 auto-routing, gated → Task 5. ✓
- Preserved GPU-packing + unreliable-device exclusion → Task 2 Step 4 (verbatim) + unchanged tests in Step 6. ✓

**Placeholder scan:** Tasks 1–4 contain full code/commands. Task 5 is explicitly contingent and parameterized on one recorded value (the RPC device token from Task 4 Step 4) — a known unknown surfaced by the gate, not a lazy TODO. The allocation math is specified.

**Type consistency:** `Plan.remote_cpu_pool_mib: u64` and `Plan.rpc_cpu_nodes: Vec<String>` used consistently across Tasks 2 and 5; `cpu_pool_mib` redefined to host-CPU and asserted as such; `host(...)`/`cpu_worker(...)`/`node(...)`/`gpu(...)` helpers consistent across tests.

---

## Execution Handoff

Pick an execution approach when ready: **Subagent-Driven** (fresh subagent per task with review between) or **Inline** (execute here with checkpoints). Tasks 1–3 are the shippable core; Task 4 is the decision gate; Task 5 runs only if Task 4 passes.

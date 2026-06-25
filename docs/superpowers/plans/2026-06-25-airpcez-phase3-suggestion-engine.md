# airpcez Phase 3 — Suggestion / Pre-flight Engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Given the live cluster + a model's metadata, suggest safe launch settings (`-ngl`, tensor-split ratio, `--cpu-moe`), flag broken devices to exclude, and return a fit verdict (✅ fits / ⚠️ tight / ❌ won't fit + bottleneck) — so launches stop failing the OOM trial-and-error this whole project was born from.

**Architecture:** Builds on Phase 2. A NEW pure `airpcez-core` module (`planner.rs`) holds `ModelMeta`, the `Plan`/`FitVerdict` types, and `suggest_plan(cluster, meta, ctx) -> Plan` — all OS-free and unit-tested, with the **70B OOM saga encoded as a regression fixture**. The binary adds a tiny model catalog, a `POST /suggest` endpoint (gathers the cluster like `/cluster` does, runs the planner), and a "Suggest settings" + fit-verdict pre-flight in the cockpit UI.

**Tech Stack:** Same as Phase 2 (Rust 2021/stable, tokio, axum, serde, reqwest). No new deps.

## Global Constraints

- Rust edition 2021; must build on stable. `cargo build` AND `cargo test` PRISTINE (0 warnings).
- `airpcez-core` MUST remain OS-free (no tokio/std::process/Command/reqwest/cfg(target_os)). The planner is pure.
- Reuse Phase 1/2 interfaces verbatim — do not redefine:
  - `airpcez_core::model::{NodeStats, DeviceStats { name, kind, vram_total_mib, vram_free_mib, reliable }, vram_reliable}`.
  - `airpcez_core::cluster::{NodeEntry, NodeSnapshot { entry, stats: Option<NodeStats>, reachable, error }, ClusterStatus { nodes, warnings }}`. The host's own node is `nodes[0]` (addr `"self"`), workers follow — this is the device-enumeration order llama.cpp uses (local GPUs first, then RPC workers).
  - `airpcez_core::flags::{CpuMoe}` (the launch builder; the planner outputs map to it).
  - Phase 2 endpoints exist: `GET /cluster`, `POST /host/launch` (`LaunchRequest { model_hf?, model_path?, ngl?, tensor_split?, main_gpu?, device?, cpu_moe?, ctx? }`). The UI's Run-a-Model form is where the suggestion plugs in.
- The planner is a documented HEURISTIC (v1): it never claims VRAM it can't see, always excludes devices flagged `reliable == false` (the Vulkan bogus-VRAM lesson), and reserves per-tier headroom.

---

### Task 1: Core planner types

**Files:**
- Create: `crates/airpcez-core/src/planner.rs`
- Modify: `crates/airpcez-core/src/lib.rs` (`pub mod planner;`)

**Interfaces:**
- Consumes: nothing from earlier Phase-3 tasks.
- Produces:
  - `struct ModelMeta { total_mib: u64, n_layers: u32, is_moe: bool }`
  - `enum Fit { Fits, Tight, WontFit }` (`#[serde(rename_all="lowercase")]`)
  - `struct FitVerdict { fit: Fit, detail: String }`
  - `struct Plan { ngl: u32, tensor_split: Option<String>, cpu_moe: Option<String>, exclude_notes: Vec<String>, fit: FitVerdict, gpu_pool_mib: u64, cpu_pool_mib: u64 }`
  - All `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]`.

- [ ] **Step 1: Write the failing test**

`crates/airpcez-core/src/planner.rs`:
```rust
use crate::cluster::ClusterStatus;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_json_roundtrips() {
        let p = Plan {
            ngl: 40, tensor_split: Some("12,11".into()), cpu_moe: None,
            exclude_notes: vec!["drop Vulkan0".into()],
            fit: FitVerdict { fit: Fit::Tight, detail: "tight".into() },
            gpu_pool_mib: 21000, cpu_pool_mib: 26000,
        };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(p, serde_json::from_str::<Plan>(&j).unwrap());
        assert!(j.contains("\"fit\":\"tight\""));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez-core plan_json_roundtrips`
Expected: FAIL (types/module missing).

- [ ] **Step 3: Write minimal implementation**

At the top of `planner.rs`:
```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ModelMeta { pub total_mib: u64, pub n_layers: u32, pub is_moe: bool }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Fit { Fits, Tight, WontFit }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct FitVerdict { pub fit: Fit, pub detail: String }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Plan {
    pub ngl: u32,
    pub tensor_split: Option<String>,
    pub cpu_moe: Option<String>,
    pub exclude_notes: Vec<String>,
    pub fit: FitVerdict,
    pub gpu_pool_mib: u64,
    pub cpu_pool_mib: u64,
}
```
Add `pub mod planner;` to `crates/airpcez-core/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez-core plan_json_roundtrips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/planner.rs crates/airpcez-core/src/lib.rs
git commit -m "feat(core): planner types (ModelMeta, Plan, FitVerdict)"
```

---

### Task 2: Pure planner helpers (KV estimate + ratio)

**Files:**
- Modify: `crates/airpcez-core/src/planner.rs`

**Interfaces:**
- Produces:
  - `fn kv_mib(n_layers: u32, ctx: u32) -> u64` — rough KV-cache estimate: `n_layers * ctx * 0.13 MiB/(layer·1k-tok)` simplified to integer math. (Heuristic; refined later.)
  - `fn ratio_string(parts: &[u64]) -> Option<String>` — reduce a list of MiB values to a small comma-separated tensor-split ratio (e.g. `[7300,11200,10900] -> "7,11,11"`). Returns `None` for empty/all-zero.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `planner.rs`:
```rust
#[test]
fn kv_scales_with_layers_and_ctx() {
    assert_eq!(kv_mib(0, 8192), 0);
    // 80 layers * 8192 tok: > 0 and monotonic in both inputs
    let a = kv_mib(80, 4096);
    let b = kv_mib(80, 8192);
    let c = kv_mib(40, 8192);
    assert!(a > 0 && b > a && b > c);
}

#[test]
fn ratio_reduces_to_small_ints() {
    assert_eq!(ratio_string(&[7300, 11200, 10900]).as_deref(), Some("7,11,11"));
    assert_eq!(ratio_string(&[8000]).as_deref(), Some("8"));
    assert_eq!(ratio_string(&[]), None);
    assert_eq!(ratio_string(&[0, 0]), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p airpcez-core kv_scales ratio_reduces`
Expected: FAIL (fns missing).

- [ ] **Step 3: Write minimal implementation**

Add to `planner.rs`:
```rust
/// Rough KV-cache size: ~0.125 MiB per layer per 1024 context tokens.
/// A heuristic for the fit check, not an exact figure.
pub fn kv_mib(n_layers: u32, ctx: u32) -> u64 {
    (n_layers as u64 * ctx as u64) / 8192
}

/// Reduce MiB values to a small comma-separated ratio for --tensor-split,
/// by rounding each to the nearest GiB (clamped to >=1). None if all zero.
pub fn ratio_string(parts: &[u64]) -> Option<String> {
    if parts.iter().copied().max().unwrap_or(0) == 0 { return None; }
    let scaled: Vec<u64> = parts.iter().map(|&p| ((p + 512) / 1024).max(1)).collect();
    Some(scaled.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p airpcez-core kv_scales ratio_reduces`
Expected: PASS. (If `ratio_string([7300,11200,10900])` differs from `"7,11,11"`, adjust the rounding so 7300→7, 11200→11, 10900→11 — verify the exact expected string and keep it.)

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/planner.rs
git commit -m "feat(core): planner helpers kv_mib + ratio_string"
```

---

### Task 3: The planner + the 70B regression fixture (centerpiece)

**Files:**
- Modify: `crates/airpcez-core/src/planner.rs`

**Interfaces:**
- Consumes: `ClusterStatus`, `ModelMeta`, `kv_mib`, `ratio_string`, the `Plan`/`Fit` types.
- Produces: `fn suggest_plan(cluster: &ClusterStatus, meta: &ModelMeta, ctx: u32) -> Plan`.

The algorithm (heuristic; documented in code):
1. Walk `cluster.nodes` in order (self first). For each reachable node's devices: if `reliable && vram_total_mib > 0`, add `vram_free_mib.saturating_sub(GPU_HEADROOM_MIB)` to the GPU pool and to the split list (this is host-then-worker order, matching llama.cpp's enumeration); if `!reliable`, push an exclude note naming the device.
2. CPU pool = sum over reachable nodes of `ram_free_mib.saturating_sub(CPU_HEADROOM_MIB)`.
3. `required = meta.total_mib + kv_mib(n_layers, ctx)`.
4. `ngl = min(n_layers, gpu_pool / per_layer)` where `per_layer = max(1, total_mib / max(1, n_layers))`.
5. `tensor_split = ratio_string(splits)`.
6. `cpu_moe = is_moe.then(|| if gpu_pool >= total_mib { "off" } else { "all" })`.
7. Fit: `Fits` if `required + required/10 <= gpu_pool + cpu_pool`; `Tight` if `required <= pool`; else `WontFit`. detail names the shortfall/headroom.

- [ ] **Step 1: Write the failing tests (a simple case + the 70B regression fixture)**

Add to the `tests` module in `planner.rs`:
```rust
use crate::cluster::{NodeEntry, NodeSnapshot};
use crate::model::{DeviceKind, DeviceStats, NodeStats, Role};

fn node(name: &str, ram_free: u64, devices: Vec<DeviceStats>) -> NodeSnapshot {
    NodeSnapshot {
        entry: NodeEntry { name: name.into(), addr: format!("{name}:8675") },
        stats: Some(NodeStats {
            name: name.into(), role: Role::Worker, ram_total_mib: ram_free + 4000,
            ram_free_mib: ram_free, cpu_logical: 8, devices, rpc_endpoint: None,
            binary_version: Some("b9789".into()), running: false, sampled_at_unix: 0,
        }),
        reachable: true, error: None,
    }
}
fn gpu(name: &str, kind: DeviceKind, total: u64, free: u64, reliable: bool) -> DeviceStats {
    DeviceStats { name: name.into(), kind, vram_total_mib: total, vram_free_mib: free, reliable }
}

#[test]
fn plan_fits_small_model_on_one_gpu() {
    // One node, one healthy 12 GB GPU; a 6 GB / 32-layer dense model.
    let cluster = ClusterStatus {
        nodes: vec![node("mac", 8000, vec![gpu("MTL0", DeviceKind::Metal, 12000, 11000, true)])],
        warnings: vec![],
    };
    let meta = ModelMeta { total_mib: 6000, n_layers: 32, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);
    assert_eq!(p.fit.fit, Fit::Fits);
    assert_eq!(p.ngl, 32);            // whole model fits on GPU
    assert!(p.exclude_notes.is_empty());
}

#[test]
fn plan_excludes_bogus_vram_and_sizes_to_real_free_70b() {
    // The actual 70B saga: Linux 2080 Super reports a BOGUS (unreliable) VRAM value,
    // M2 (12 GB Metal) + M1 (~11 GB Metal) are reliable, Linux has ~28 GB CPU free.
    // 70B Q4 ~= 42_000 MiB, 80 dense layers.
    let cluster = ClusterStatus {
        nodes: vec![
            node("linux-2080", 28000, vec![gpu("Vulkan0", DeviceKind::Cuda, 8438, 17_592_186_044_362, false)]),
            node("m2",         13000, vec![gpu("MTL0", DeviceKind::Metal, 12000, 12000, true)]),
            node("m1",         13000, vec![gpu("MTL0", DeviceKind::Metal, 11000, 10900, true)]),
        ],
        warnings: vec![],
    };
    let meta = ModelMeta { total_mib: 42_000, n_layers: 80, is_moe: false };
    let p = suggest_plan(&cluster, &meta, 4096);

    // The broken Vulkan device is excluded and called out.
    assert_eq!(p.exclude_notes.len(), 1);
    assert!(p.exclude_notes[0].contains("Vulkan0"));
    // GPU pool counts ONLY the two reliable Metal GPUs (minus headroom), NOT the bogus one.
    assert!(p.gpu_pool_mib > 18_000 && p.gpu_pool_mib < 22_000);
    // ngl is bounded by the real GPU pool, not the whole model.
    assert!(p.ngl > 25 && p.ngl < 50);
    // tensor-split is over the two reliable GPUs only (two entries).
    assert_eq!(p.tensor_split.as_deref().unwrap().split(',').count(), 2);
    // It DOES fit once CPU is counted (this is the config that finally worked).
    assert!(matches!(p.fit.fit, Fit::Fits | Fit::Tight));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p airpcez-core plan_fits_small plan_excludes_bogus`
Expected: FAIL (`suggest_plan` missing).

- [ ] **Step 3: Write minimal implementation**

Add to `planner.rs`:
```rust
const GPU_HEADROOM_MIB: u64 = 1024;
const CPU_HEADROOM_MIB: u64 = 2048;

/// Heuristic launch planner. Never trusts a device flagged `reliable == false`
/// (e.g. the Vulkan overflow); reserves per-tier headroom; sizes -ngl to the
/// real reliable GPU pool and verdicts the fit against GPU + CPU memory.
pub fn suggest_plan(cluster: &ClusterStatus, meta: &ModelMeta, ctx: u32) -> Plan {
    let mut gpu_pool = 0u64;
    let mut splits: Vec<u64> = Vec::new();
    let mut exclude_notes = Vec::new();
    let mut cpu_pool = 0u64;
    for n in &cluster.nodes {
        let Some(st) = &n.stats else { continue };
        if !n.reachable { continue; }
        let mut unified_vram = 0u64; // Apple-Silicon Metal VRAM is carved from system RAM
        for d in &st.devices {
            if d.reliable && d.vram_total_mib > 0 {
                let usable = d.vram_free_mib.saturating_sub(GPU_HEADROOM_MIB);
                gpu_pool += usable;
                splits.push(usable);
                if matches!(d.kind, crate::model::DeviceKind::Metal) {
                    unified_vram += d.vram_free_mib;
                }
            } else if !d.reliable {
                exclude_notes.push(format!(
                    "{}/{}: unreliable VRAM reading ({} MiB) — exclude this device",
                    n.entry.name, d.name, d.vram_free_mib));
            }
        }
        // Unified-memory nodes (Metal): don't double-count VRAM as free CPU RAM.
        cpu_pool += st.ram_free_mib.saturating_sub(CPU_HEADROOM_MIB + unified_vram);
    }
    let per_layer = (meta.total_mib / meta.n_layers.max(1) as u64).max(1);
    let ngl = ((gpu_pool / per_layer) as u32).min(meta.n_layers);
    let tensor_split = ratio_string(&splits);
    let cpu_moe = if meta.is_moe {
        Some(if gpu_pool >= meta.total_mib { "off".to_string() } else { "all".to_string() })
    } else { None };

    let required = meta.total_mib + kv_mib(meta.n_layers, ctx);
    let pool = gpu_pool + cpu_pool;
    let (fit, detail) = if required + required / 10 <= pool {
        (Fit::Fits, format!("fits — ~{} MiB headroom across {} MiB GPU + {} MiB CPU",
            pool.saturating_sub(required), gpu_pool, cpu_pool))
    } else if required <= pool {
        (Fit::Tight, format!("tight — needs {} MiB, pool is {} MiB ({} GPU + {} CPU)",
            required, pool, gpu_pool, cpu_pool))
    } else {
        (Fit::WontFit, format!("won't fit — needs {} MiB but pool is only {} MiB ({} GPU + {} CPU); add memory or use a smaller quant",
            required, pool, gpu_pool, cpu_pool))
    };
    Plan { ngl, tensor_split, cpu_moe, exclude_notes,
        fit: FitVerdict { fit, detail }, gpu_pool_mib: gpu_pool, cpu_pool_mib: cpu_pool }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p airpcez-core suggest_plan plan_fits_small plan_excludes_bogus`
Expected: PASS. (If `gpu_pool`/`ngl` land just outside the fixture bounds, the bounds are intentionally loose — confirm the numbers and adjust the assertion bounds, NOT the headroom constants, only if clearly off.)

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/planner.rs
git commit -m "feat(core): suggest_plan heuristic + 70B regression fixture"
```

---

### Task 4: Model catalog (UI pre-fill)

**Files:**
- Create: `crates/airpcez/src/catalog.rs`
- Modify: `crates/airpcez/src/lib.rs` (`pub mod catalog;`)

**Interfaces:**
- Consumes: `ModelMeta`.
- Produces: `struct CatalogEntry { label: String, hf: String, meta: ModelMeta }` (serde) and `fn model_catalog() -> Vec<CatalogEntry>` — a few known models with their approximate Q4 size + layer count + MoE flag.

- [ ] **Step 1: Write the failing test**

`crates/airpcez/src/catalog.rs`:
```rust
use airpcez_core::planner::ModelMeta;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_has_known_models_with_sane_meta() {
        let c = model_catalog();
        assert!(c.len() >= 3);
        let moe = c.iter().find(|e| e.hf.contains("35B-A3B")).unwrap();
        assert!(moe.meta.is_moe && moe.meta.n_layers > 0 && moe.meta.total_mib > 10_000);
        let dense70 = c.iter().find(|e| e.hf.contains("70B")).unwrap();
        assert!(!dense70.meta.is_moe && dense70.meta.total_mib > 35_000);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez catalog_has_known_models`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct CatalogEntry { pub label: String, pub hf: String, pub meta: ModelMeta }

/// Approximate Q4_K_M metadata for a few known models. Sizes in MiB.
pub fn model_catalog() -> Vec<CatalogEntry> {
    let e = |label: &str, hf: &str, total_mib: u64, n_layers: u32, is_moe: bool| CatalogEntry {
        label: label.into(), hf: hf.into(),
        meta: ModelMeta { total_mib, n_layers, is_moe },
    };
    vec![
        e("Qwen3.6-27B (dense) Q4_K_M",  "unsloth/Qwen3.6-27B-GGUF:Q4_K_M",      17_000, 64, false),
        e("Qwen3.6-35B-A3B (MoE) Q4_K_M","unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M",  21_000, 48, true),
        e("Llama-3.3-70B Q4_K_M",        "unsloth/Llama-3.3-70B-Instruct-GGUF:Q4_K_M", 42_000, 80, false),
    ]
}
```
Add `pub mod catalog;` to `crates/airpcez/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez catalog_has_known_models`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/catalog.rs crates/airpcez/src/lib.rs
git commit -m "feat: model catalog for suggestion pre-fill"
```

---

### Task 5: `POST /suggest` + `GET /catalog`

**Files:**
- Modify: `crates/airpcez/src/server.rs`

**Interfaces:**
- Consumes: `suggest_plan`, `ModelMeta`, `poll_nodes`, `model_catalog`, `AppState`.
- Produces:
  - `GET /catalog` → `Vec<CatalogEntry>` JSON.
  - `POST /suggest` (request `{ meta: ModelMeta, ctx: u32 }`) → `Plan` JSON. It builds the cluster exactly like `/cluster` (self snapshot at index 0 + `poll_nodes(&http, &nodes)`) and runs `suggest_plan`.

- [ ] **Step 1: Write the failing test**

`crates/airpcez/tests/suggest_endpoint.rs`:
```rust
use airpcez_core::planner::{Plan, ModelMeta};
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn suggest_returns_a_plan_for_self() {
    // Host self has one reliable 12 GB GPU; ask for a small model.
    let stats = NodeStats {
        name: "host".into(), role: Role::Host, ram_total_mib: 16000, ram_free_mib: 9000,
        cpu_logical: 8,
        devices: vec![DeviceStats { name: "MTL0".into(), kind: DeviceKind::Metal,
            vram_total_mib: 12000, vram_free_mib: 11000, reliable: true }],
        rpc_endpoint: None, binary_version: None, running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19201, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let plan: Plan = reqwest::Client::new().post("http://127.0.0.1:19201/suggest")
        .json(&serde_json::json!({ "meta": ModelMeta { total_mib: 6000, n_layers: 32, is_moe: false }, "ctx": 4096 }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(plan.ngl, 32);
    assert!(plan.gpu_pool_mib > 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez suggest_returns_a_plan_for_self`
Expected: FAIL (route missing).

- [ ] **Step 3: Write minimal implementation**

Add routes `.route("/catalog", get(catalog_handler)).route("/suggest", post(suggest_handler))` and:
```rust
async fn catalog_handler() -> Json<Vec<crate::catalog::CatalogEntry>> {
    Json(crate::catalog::model_catalog())
}

#[derive(serde::Deserialize)]
struct SuggestRequest { meta: airpcez_core::planner::ModelMeta, ctx: u32 }

async fn suggest_handler(State(s): State<AppState>, Json(req): Json<SuggestRequest>)
    -> Json<airpcez_core::planner::Plan> {
    use airpcez_core::cluster::*;
    let self_stats = s.provider.sample();
    let self_snap = NodeSnapshot {
        entry: NodeEntry { name: self_stats.name.clone(), addr: "self".into() },
        stats: Some(self_stats), reachable: true, error: None,
    };
    let nodes = { s.nodes.lock().unwrap().clone() };
    let mut cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    cluster.nodes.insert(0, self_snap);
    Json(airpcez_core::planner::suggest_plan(&cluster, &req.meta, req.ctx))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez suggest_returns_a_plan_for_self` and full `cargo test -p airpcez`
Expected: PASS, pristine.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/tests/suggest_endpoint.rs
git commit -m "feat: GET /catalog + POST /suggest (run the planner over the live cluster)"
```

---

### Task 6: Cockpit — Suggest button + fit-verdict pre-flight

**Files:**
- Modify: `crates/airpcez/assets/index.html`

**Interfaces:**
- Consumes: `GET /catalog`, `POST /suggest`, and the existing Run-a-Model form + `POST /host/launch`.

Manual-verified UI task. In the Run-a-Model panel:
- On load, `fetch('/catalog')` → populate a model dropdown (label → hf). Selecting an entry fills the `-hf` field AND stashes its `ModelMeta` (a custom "(manual)" option lets the user type an hf/path + size/layers/MoE by hand).
- Add a **Suggest settings** button: POST `/suggest` with `{ meta, ctx: <the ctx field, default 4096> }`. On the returned `Plan`: pre-fill `-ngl` (`plan.ngl`), `--tensor-split` (`plan.tensor_split`), and the `--cpu-moe` selector (`plan.cpu_moe`: "all"→all, "off"/null→off). Render a **fit verdict** chip from `plan.fit.fit` (✅ green "fits" / ⚠️ amber "tight" / ❌ red "won't fit") with `plan.fit.detail`, and list `plan.exclude_notes` (the broken-device warnings) below it.
- Show the fit chip next to the **Launch** button as a pre-flight: if `won't fit`, keep Launch enabled but visibly warn (don't hard-block — the heuristic can be wrong). Escape all interpolated strings with the existing `escapeHtml`.
- Keep everything from Phase 2 intact.

- [ ] **Step 1: Implement the UI**

Write the dropdown + Suggest button + fit-verdict rendering per above. Plain HTML/JS, reuse the existing `escapeHtml`.

- [ ] **Step 2: Manual smoke**

Run: `cargo run -p airpcez` → open `http://localhost:8675`. Confirm: the model dropdown is populated from `/catalog`; clicking **Suggest** posts to `/suggest`, fills `-ngl`/`tensor-split`/`cpu-moe`, and shows a fit chip + (on this single host) no exclude notes; no console errors. Stop the server. Then `cargo test -p airpcez` (all pass) + `cargo build -p airpcez` (0 warnings).

- [ ] **Step 3: Commit**

```bash
git add crates/airpcez/assets/index.html
git commit -m "feat: cockpit Suggest button + fit-verdict pre-flight"
```

---

## Phase 3 Definition of Done

`cargo test`/`cargo build` green and pristine. In the cockpit, picking a model and clicking **Suggest** fills `-ngl`/`--tensor-split`/`--cpu-moe` from the live cluster, flags any broken (unreliable-VRAM) device to exclude, and shows a ✅/⚠️/❌ fit verdict before you Launch. The 70B OOM saga is locked in as a regression test asserting the engine excludes the bogus Vulkan device and sizes to real free memory. This is the differentiator the project was built for — the OOM trial-and-error is replaced by a pre-flight check.

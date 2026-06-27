# Cluster Memory-Totals Strip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show aggregate System RAM / VRAM / Pool (each free + total) at the top of the cockpit's cluster panel, with Pool de-duplicating unified memory.

**Architecture:** A pure function in `airpcez-core` computes the totals from the polled `ClusterStatus`; a thin `ClusterResponse` wrapper (flatten) carries them in the `/cluster` JSON without touching the ~17 existing `ClusterStatus` constructions; the cockpit renders a 3-cell strip.

**Tech Stack:** Rust (`airpcez-core`, `axum`), vanilla JS/HTML/CSS (embedded `index.html`).

## Global Constraints

- **Pool de-dup rule:** `pool = Σ ram_total + Σ vram_total of discrete GPUs only`. Discrete = `kind ∈ {Cuda, Other}`; `Metal` is unified (its VRAM is already inside RAM) and is **excluded** from the pool add. Same rule for the `free` figures.
- **Reliability:** skip any device with `reliable == false` from VRAM and Pool (the Vulkan bogus/overflow-VRAM guard).
- **Scope of the sum:** reachable nodes that have stats, **including the host ("self")**.
- **No node-side schema change:** unified-ness is inferred from the existing `DeviceKind`.
- **JSON field names** (snake_case, consumed by the cockpit): `ram_total_mib`, `ram_free_mib`, `vram_total_mib`, `vram_free_mib`, `pool_total_mib`, `pool_free_mib`.
- Branch: `dashboard-pool-totals`.

Relevant existing types (`airpcez-core/src/model.rs`): `NodeStats { ram_total_mib, ram_free_mib, devices: Vec<DeviceStats>, .. }`, `DeviceStats { kind: DeviceKind, vram_total_mib, vram_free_mib, reliable }`, `DeviceKind { Cuda, Metal, Cpu, Other }`. `cluster.rs`: `ClusterStatus { nodes: Vec<NodeSnapshot>, warnings: Vec<String> }`, `NodeSnapshot { entry, stats: Option<NodeStats>, reachable, error }`.

---

### Task 1: Pure totals function + `ClusterResponse` wrapper (airpcez-core)

**Files:**
- Modify: `crates/airpcez-core/src/cluster.rs` (add `MemoryTotals`, `cluster_memory_totals`, `ClusterResponse`, and unit tests)

**Interfaces:**
- Produces:
  - `pub struct MemoryTotals { pub ram_total_mib: u64, pub ram_free_mib: u64, pub vram_total_mib: u64, pub vram_free_mib: u64, pub pool_total_mib: u64, pub pool_free_mib: u64 }` (derives `Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default`)
  - `pub fn cluster_memory_totals(status: &ClusterStatus) -> MemoryTotals`
  - `pub struct ClusterResponse { #[serde(flatten)] pub status: ClusterStatus, pub totals: MemoryTotals }` (derives `Serialize, Deserialize, Clone, PartialEq, Debug`)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/airpcez-core/src/cluster.rs` (the module already imports `use super::*;` and `use crate::model::Role;` — also add `use crate::model::{DeviceStats, DeviceKind, NodeStats}` if not already in scope):

```rust
fn node(name: &str, reachable: bool, ram_t: u64, ram_f: u64, devs: Vec<DeviceStats>) -> NodeSnapshot {
    NodeSnapshot {
        entry: NodeEntry { name: name.into(), addr: format!("{name}:8675") },
        stats: reachable.then(|| NodeStats {
            name: name.into(), role: Role::Worker, ram_total_mib: ram_t, ram_free_mib: ram_f,
            cpu_logical: 1, devices: devs, rpc_endpoint: None, binary_version: None,
            running: false, sampled_at_unix: 0,
        }),
        reachable, error: None,
    }
}
fn dev(kind: DeviceKind, vt: u64, vf: u64, reliable: bool) -> DeviceStats {
    DeviceStats { name: "d".into(), kind, vram_total_mib: vt, vram_free_mib: vf, reliable }
}

#[test]
fn totals_dedup_unified_skip_unreliable_and_unreachable() {
    let cs = ClusterStatus {
        nodes: vec![
            node("apple", true, 16384, 8000, vec![dev(DeviceKind::Metal, 12288, 6000, true)]),
            node("nvidia", true, 32000, 20000, vec![dev(DeviceKind::Cuda, 8192, 7000, true)]),
            node("bad-vram", true, 16000, 4000, vec![dev(DeviceKind::Cuda, 8192, 9_999_999, false)]),
            node("offline", false, 99999, 99999, vec![]),
        ],
        warnings: vec![],
    };
    let t = cluster_memory_totals(&cs);
    // RAM: apple + nvidia + bad-vram (offline contributes nothing)
    assert_eq!(t.ram_total_mib, 16384 + 32000 + 16000);
    assert_eq!(t.ram_free_mib, 8000 + 20000 + 4000);
    // VRAM: apple Metal + nvidia Cuda; bad-vram device is unreliable -> excluded
    assert_eq!(t.vram_total_mib, 12288 + 8192);
    assert_eq!(t.vram_free_mib, 6000 + 7000);
    // Pool: all RAM + ONLY the discrete (Cuda) VRAM; Metal not added, unreliable not added
    assert_eq!(t.pool_total_mib, (16384 + 32000 + 16000) + 8192);
    assert_eq!(t.pool_free_mib, (8000 + 20000 + 4000) + 7000);
}

#[test]
fn totals_empty_cluster_is_zero() {
    let cs = ClusterStatus { nodes: vec![], warnings: vec![] };
    assert_eq!(cluster_memory_totals(&cs), MemoryTotals::default());
}

#[test]
fn cluster_response_flattens_nodes_and_carries_totals() {
    let cs = ClusterStatus {
        nodes: vec![node("apple", true, 16384, 8000, vec![dev(DeviceKind::Metal, 12288, 6000, true)])],
        warnings: vec![],
    };
    let totals = cluster_memory_totals(&cs);
    let resp = ClusterResponse { status: cs, totals };
    let j = serde_json::to_value(&resp).unwrap();
    // flatten: nodes + warnings sit at top level, alongside totals
    assert!(j.get("nodes").is_some(), "nodes should be top-level (flattened)");
    assert_eq!(j["totals"]["pool_total_mib"], 16384); // Metal unified -> pool == ram
    // round-trips back into the wrapper
    assert_eq!(resp, serde_json::from_value::<ClusterResponse>(j).unwrap());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p airpcez-core --lib cluster::`
Expected: FAIL — `cannot find type MemoryTotals` / `cannot find function cluster_memory_totals` / `cannot find type ClusterResponse`.

- [ ] **Step 3: Implement the types + function**

Add to `crates/airpcez-core/src/cluster.rs` (top-level, after the `ClusterStatus` struct; `use crate::model::DeviceKind;` is needed — add it to the existing `use crate::model::...` line at the top of the file):

```rust
/// Aggregate cluster memory. `pool_*` de-duplicates unified memory: an Apple-Silicon
/// node's Metal VRAM is carved from its system RAM, so it is counted once (in RAM),
/// while a discrete GPU's VRAM (Cuda/Other) is a separate pool and is added on top.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MemoryTotals {
    pub ram_total_mib: u64,
    pub ram_free_mib: u64,
    pub vram_total_mib: u64,
    pub vram_free_mib: u64,
    pub pool_total_mib: u64,
    pub pool_free_mib: u64,
}

/// Sum memory across reachable nodes that have stats (the host's "self" node included).
/// VRAM/Pool skip devices flagged unreliable. Pool adds only DISCRETE GPU VRAM
/// (`kind != Metal && kind != Cpu`); `Metal` is unified and already inside RAM.
pub fn cluster_memory_totals(status: &ClusterStatus) -> MemoryTotals {
    let mut t = MemoryTotals::default();
    for n in &status.nodes {
        if !n.reachable { continue; }
        let Some(s) = n.stats.as_ref() else { continue; };
        t.ram_total_mib += s.ram_total_mib;
        t.ram_free_mib += s.ram_free_mib;
        t.pool_total_mib += s.ram_total_mib;
        t.pool_free_mib += s.ram_free_mib;
        for d in &s.devices {
            if !d.reliable || d.kind == DeviceKind::Cpu { continue; }
            t.vram_total_mib += d.vram_total_mib;
            t.vram_free_mib += d.vram_free_mib;
            if d.kind != DeviceKind::Metal {
                t.pool_total_mib += d.vram_total_mib;
                t.pool_free_mib += d.vram_free_mib;
            }
        }
    }
    t
}

/// The `/cluster` response: `ClusterStatus` (flattened) plus computed `totals`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ClusterResponse {
    #[serde(flatten)]
    pub status: ClusterStatus,
    pub totals: MemoryTotals,
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p airpcez-core --lib cluster::`
Expected: PASS (3 new tests + the existing `cluster::` tests), output pristine.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/cluster.rs
git commit -m "feat(core): cluster_memory_totals + ClusterResponse (unified-dedup pool)"
```

---

### Task 2: Carry totals in the `/cluster` endpoint

**Files:**
- Modify: `crates/airpcez/src/server.rs` (`cluster_handler`, ~line 96)
- Test: `crates/airpcez/tests/cluster_endpoint.rs`

**Interfaces:**
- Consumes: `airpcez_core::cluster::{cluster_memory_totals, ClusterResponse}` from Task 1.

- [ ] **Step 1: Update the failing test**

In `crates/airpcez/tests/cluster_endpoint.rs`, change the import and the deserialization target + add totals assertions. Replace the body so it reads:

```rust
use airpcez_core::cluster::ClusterResponse;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn cluster_endpoint_includes_self_and_totals() {
    let stats = NodeStats { name: "host".into(), role: Role::Host, ram_total_mib: 16,
        ram_free_mib: 8, cpu_logical: 8, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19102, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let cs: ClusterResponse = reqwest::get("http://127.0.0.1:19102/cluster")
        .await.unwrap().json().await.unwrap();
    assert_eq!(cs.status.nodes.len(), 1); // just self (no workers configured)
    assert_eq!(cs.status.nodes[0].entry.name, "host");
    assert!(cs.status.nodes[0].reachable && cs.status.nodes[0].stats.is_some());
    // self has 16 MiB RAM and no GPU -> pool == ram, vram == 0
    assert_eq!(cs.totals.ram_total_mib, 16);
    assert_eq!(cs.totals.ram_free_mib, 8);
    assert_eq!(cs.totals.vram_total_mib, 0);
    assert_eq!(cs.totals.pool_total_mib, 16);
    assert_eq!(cs.totals.pool_free_mib, 8);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p airpcez --test cluster_endpoint`
Expected: FAIL — the handler still returns bare `ClusterStatus` (no `totals` field), so deserializing `ClusterResponse` errors on the missing `totals` key (or the type doesn't exist yet to import).

- [ ] **Step 3: Update `cluster_handler` to return `ClusterResponse`**

In `crates/airpcez/src/server.rs`, change the `cluster_handler` signature and its final two lines. The function currently ends:
```rust
    cluster.nodes.insert(0, self_snap);
    Json(cluster)
}
```
Change the return type from `Json<airpcez_core::cluster::ClusterStatus>` to `Json<airpcez_core::cluster::ClusterResponse>`, and replace the tail with:
```rust
    cluster.nodes.insert(0, self_snap);
    let totals = cluster_memory_totals(&cluster);
    Json(ClusterResponse { status: cluster, totals })
}
```
(`use airpcez_core::cluster::*;` is already the first line of the function body, so `cluster_memory_totals` and `ClusterResponse` are in scope.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p airpcez --test cluster_endpoint`
Expected: PASS.

- [ ] **Step 5: Run the full workspace to confirm nothing regressed**

Run: `cargo test`
Expected: all pass (the flatten wrapper means existing `ClusterStatus` constructions/round-trip tests are untouched).

- [ ] **Step 6: Commit**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/tests/cluster_endpoint.rs
git commit -m "feat(server): /cluster returns ClusterResponse with memory totals"
```

---

### Task 3: Cockpit totals strip

**Files:**
- Modify: `crates/airpcez/assets/index.html` (CSS in the `<style>` block; HTML in `.cluster-panel`; JS in the `/cluster` poll handler)

**Interfaces:**
- Consumes: `data.totals.{ram,vram,pool}_{total,free}_mib` from the `/cluster` JSON (Task 2); the existing `fmtMib(mib)` helper (defined ~line 394).

- [ ] **Step 1: Add the HTML container**

In `crates/airpcez/assets/index.html`, inside `<div class="panel cluster-panel">`, immediately after `<h2>Cluster</h2>` (and before the `cluster-warnings` banner), insert:

```html
      <!-- Aggregate memory totals (rendered by JS each /cluster poll) -->
      <div class="cluster-totals" id="cluster-totals"></div>
```

- [ ] **Step 2: Add CSS**

In the `<style>` block (e.g. just after the `.cluster-panel` rule near the top), add:

```css
    .cluster-totals { display: flex; gap: 8px; padding: 0 4px; }
    .ct-cell { flex: 1; background: #0d1117; border: 1px solid #30363d; border-radius: 6px; padding: 6px 8px; }
    .ct-label { font-size: 11px; color: #8b949e; text-transform: uppercase; letter-spacing: .03em; }
    .ct-val { font-size: 12px; color: #c9d1d9; margin-top: 2px; }
    .ct-hint { color: #6e7681; text-transform: none; letter-spacing: 0; }
```

- [ ] **Step 3: Render the strip on each poll**

In the `/cluster` poll handler (the `async function` that does `const resp = await fetch("/cluster")`, ~line 544), after the response JSON is parsed into its variable (the same place that updates `cluster-warnings` and `cluster-node-list`), add a render of the totals. If the parsed object is named `data` (confirm the actual variable name in that function and use it), add:

```javascript
        // Aggregate memory totals strip
        const tot = data.totals || {};
        const ctCell = (label, free, total, hint) =>
          '<div class="ct-cell"><div class="ct-label">' + label + (hint || '') + '</div>' +
          '<div class="ct-val">' + fmtMib(free || 0) + ' free / ' + fmtMib(total || 0) + ' total</div></div>';
        const ctEl = document.getElementById("cluster-totals");
        if (ctEl) ctEl.innerHTML =
          ctCell("System RAM", tot.ram_free_mib, tot.ram_total_mib) +
          ctCell("VRAM", tot.vram_free_mib, tot.vram_total_mib) +
          ctCell("Pool", tot.pool_free_mib, tot.pool_total_mib, ' <span class="ct-hint">(dedup)</span>');
```

(If that function names the parsed body something other than `data` — e.g. `cluster` — use that name for `.totals`.)

- [ ] **Step 4: Verify end-to-end against the running host**

The host's airpcez is live on `:8675`. Rebuild/restart it per the project's normal run flow, then:

Run: `curl -s http://127.0.0.1:8675/cluster | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['totals'])"`
Expected: a JSON object with all six `*_mib` keys populated (non-zero `ram_total_mib`/`pool_total_mib`).

Then load the cockpit (`http://127.0.0.1:8675/`) and confirm the three-cell strip appears at the top of the cluster panel showing `free / total` for System RAM, VRAM, and Pool, with Pool ≤ RAM + VRAM (less, when a unified Metal node is present). Sanity-check the dedup by hand: Pool should equal Σ RAM + only the discrete (NVIDIA) VRAM.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/assets/index.html
git commit -m "feat(cockpit): cluster memory-totals strip (RAM/VRAM/Pool dedup)"
```

---

## Self-Review

**Spec coverage:**
- Pure `cluster_memory_totals` in core, unit-tested → Task 1. ✓
- De-dup rule (Pool = RAM + discrete VRAM; Metal excluded) → Task 1 Step 3 + the `totals_dedup_*` test. ✓
- Reliability exclusion → Task 1 (`if !d.reliable ... continue`) + the unreliable device in the test. ✓
- Reachable-only incl. host "self" → Task 1 (`if !n.reachable / stats.as_ref()`) + Task 2 (self_snap counted because totals computed after `insert(0, self_snap)`). ✓
- Both total and free for RAM/VRAM/Pool → `MemoryTotals` six fields; UI shows `free / total`. ✓
- `/cluster` carries totals without node-schema change → `ClusterResponse` flatten wrapper, Task 2. ✓
- UI strip at top of cluster panel → Task 3. ✓
- Mixed-cluster + empty tests; endpoint test → Task 1 + Task 2. ✓

**Placeholder scan:** No TBD/TODO. The only conditional instruction is Task 3 Step 3's "confirm the parsed body's variable name" — necessary because the exact JS variable must match the existing function; the code is otherwise complete.

**Type consistency:** `MemoryTotals` field names (`ram_total_mib`, `ram_free_mib`, `vram_total_mib`, `vram_free_mib`, `pool_total_mib`, `pool_free_mib`) are identical across the struct (Task 1), the endpoint assertions (Task 2), and the JS reads (Task 3). `cluster_memory_totals` and `ClusterResponse` names match between Task 1 (produces) and Task 2 (consumes). ✓

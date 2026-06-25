# airpcez Phase 2 — Host Role + Flag-Builder + Cockpit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an airpcez instance act as the cluster **host** — aggregate every node's live stats into one cockpit, build and launch `llama-server` across the worker `rpc-server`s, and surface the OpenAI URL.

**Architecture:** Builds on Phase 1. New pure core logic (`cluster.rs` aggregation types, a `llama_server_spec` flag-builder, a version-match helper) stays OS-free in `airpcez-core`. The binary gains a reqwest-based **poller** that fans out to each node's `/stats`, a `GET /cluster` aggregate, node add/remove endpoints, `POST /host/launch`+`/host/stop`, and a `binary_version` source; the web UI grows the host cockpit (layout A).

**Tech Stack:** Same as Phase 1 (Rust 2021/stable, tokio, axum 0.7, serde, toml, sysinfo) + `reqwest` promoted from dev-dep to a normal dep (already present at `0.12`, features `["json"]`).

## Global Constraints

- Rust edition 2021; must build on stable. Applies to every task.
- `airpcez-core` MUST remain OS-free (no `tokio`, `std::process`, `Command`, `reqwest`, `#[cfg(target_os)]`). Poller/process/HTTP code lives in the `airpcez` binary.
- Reuse Phase 1 interfaces verbatim — do not redefine them:
  - `airpcez_core::model::{NodeStats, DeviceStats, Role, DeviceKind, vram_reliable}` — `NodeStats { name, role, ram_total_mib, ram_free_mib, cpu_logical, devices: Vec<DeviceStats>, rpc_endpoint: Option<String>, binary_version: Option<String>, running: bool, sampled_at_unix: u64 }`.
  - `airpcez_core::process::{ProcSpec, ProcStatus, ProcessBackend}` (`start(spec)->Result<(),String>`, `stop()->bool`, `status()->ProcStatus`, `recent_logs()->Vec<String>`).
  - `airpcez_core::flags::rpc_server_spec(binary, host, port, device) -> ProcSpec`.
  - `airpcez_core::stats::StatsProvider { fn sample(&self) -> NodeStats }`.
  - `airpcez::config::Config { ui_port, rpc_port, llama_port, role, llama_dir: Option<String>, node_name }` (`load(&Path)`, `save(&Path)->Result<(),String>`).
  - `airpcez::server::{AppState { provider: Arc<dyn StatsProvider>, supervisor: Arc<dyn ProcessBackend> }, run_server(port, state)}`; routes `/`, `/stats`, `/ws`, `/worker/start`, `/worker/stop`.
  - `airpcez::stats_provider::LocalStats { name, role }`; `airpcez::supervisor::TokioSupervisor`.
- Default ports unchanged (UI 8675 / RPC 50052 / llama-server 8080). Build/test output PRISTINE.
- The host is itself a node: the cluster view includes the host's own local stats plus the polled workers.

---

### Task 1: Core cluster aggregation types

**Files:**
- Create: `crates/airpcez-core/src/cluster.rs`
- Modify: `crates/airpcez-core/src/lib.rs` (add `pub mod cluster;`)

**Interfaces:**
- Consumes: `NodeStats` (Phase 1).
- Produces:
  - `struct NodeEntry { name: String, addr: String }` (addr = a worker's airpcez `host:ui_port`, e.g. `"192.168.0.125:8675"`).
  - `struct NodeSnapshot { entry: NodeEntry, stats: Option<NodeStats>, reachable: bool, error: Option<String> }`.
  - `struct ClusterStatus { nodes: Vec<NodeSnapshot> }`.
  - All `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]`.

- [ ] **Step 1: Write the failing test**

`crates/airpcez-core/src/cluster.rs`:
```rust
use crate::model::NodeStats;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;
    #[test]
    fn cluster_status_json_roundtrips() {
        let cs = ClusterStatus {
            nodes: vec![NodeSnapshot {
                entry: NodeEntry { name: "linux-2080".into(), addr: "192.168.0.24:8675".into() },
                stats: Some(NodeStats {
                    name: "linux-2080".into(), role: Role::Worker, ram_total_mib: 32000,
                    ram_free_mib: 18000, cpu_logical: 16, devices: vec![], rpc_endpoint:
                    Some("192.168.0.24:50052".into()), binary_version: Some("b9789".into()),
                    running: true, sampled_at_unix: 1,
                }),
                reachable: true, error: None,
            }],
            warnings: vec![],
        };
        let j = serde_json::to_string(&cs).unwrap();
        assert_eq!(cs, serde_json::from_str::<ClusterStatus>(&j).unwrap());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez-core cluster_status_json_roundtrips`
Expected: FAIL (types/module missing).

- [ ] **Step 3: Write minimal implementation**

At the top of `cluster.rs`:
```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct NodeEntry { pub name: String, pub addr: String }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct NodeSnapshot {
    pub entry: NodeEntry,
    pub stats: Option<NodeStats>,
    pub reachable: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ClusterStatus {
    pub nodes: Vec<NodeSnapshot>,
    #[serde(default)]
    pub warnings: Vec<String>,
}
```
Add `pub mod cluster;` to `crates/airpcez-core/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez-core cluster_status_json_roundtrips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/cluster.rs crates/airpcez-core/src/lib.rs
git commit -m "feat(core): cluster aggregation types (NodeEntry, NodeSnapshot, ClusterStatus)"
```

---

### Task 2: Node list in Config

**Files:**
- Modify: `crates/airpcez/src/config.rs`
- Test: `crates/airpcez/tests/config.rs`

**Interfaces:**
- Consumes: `NodeEntry` (Task 1).
- Produces: `Config.nodes: Vec<NodeEntry>` (defaults to empty), serialized in the TOML.

- [ ] **Step 1: Write the failing test**

Add to `crates/airpcez/tests/config.rs`:
```rust
#[test]
fn config_persists_nodes() {
    use airpcez_core::cluster::NodeEntry;
    let dir = std::env::temp_dir().join(format!("airpcez-nodes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut c = airpcez::config::Config::load(&path);
    assert!(c.nodes.is_empty());
    c.nodes.push(NodeEntry { name: "m2".into(), addr: "192.168.0.125:8675".into() });
    c.save(&path).unwrap();
    let c2 = airpcez::config::Config::load(&path);
    assert_eq!(c2.nodes.len(), 1);
    assert_eq!(c2.nodes[0].addr, "192.168.0.125:8675");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez config_persists_nodes`
Expected: FAIL (no `nodes` field).

- [ ] **Step 3: Write minimal implementation**

In `config.rs`, add to the struct: `pub nodes: Vec<airpcez_core::cluster::NodeEntry>,` and to `Default`: `nodes: Vec::new(),`. Add `#[serde(default)]` on the field so older config files (without `nodes`) still load:
```rust
#[serde(default)]
pub nodes: Vec<airpcez_core::cluster::NodeEntry>,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez config_persists_nodes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/config.rs crates/airpcez/tests/config.rs
git commit -m "feat: Config.nodes list (worker addresses)"
```

---

### Task 3: `llama_server_spec` flag-builder (the centerpiece)

**Files:**
- Modify: `crates/airpcez-core/src/flags.rs`

**Interfaces:**
- Consumes: `ProcSpec`.
- Produces:
  - `enum ModelRef { Hf(String), Local(String) }`
  - `enum CpuMoe { Off, All, NLayers(u32) }`
  - `struct LlamaServerOpts<'a> { binary: &'a str, model: &'a ModelRef, rpc_endpoints: &'a [String], ngl: Option<u32>, tensor_split: Option<&'a str>, main_gpu: Option<u32>, device: Option<&'a str>, cpu_moe: &'a CpuMoe, ctx: Option<u32>, host: &'a str, port: u16 }`
  - `fn llama_server_spec(opts: &LlamaServerOpts) -> ProcSpec`

- [ ] **Step 1: Write the failing golden tests**

Add to the `tests` module in `flags.rs`:
```rust
#[test]
fn builds_moe_solo_argv() {
    let model = ModelRef::Hf("unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M".into());
    let opts = LlamaServerOpts {
        binary: "/llama/llama-server", model: &model, rpc_endpoints: &[],
        ngl: Some(99), tensor_split: None, main_gpu: None, device: None,
        cpu_moe: &CpuMoe::All, ctx: Some(8192), host: "0.0.0.0", port: 8080,
    };
    let spec = llama_server_spec(&opts);
    assert_eq!(spec.program, "/llama/llama-server");
    assert_eq!(spec.args, vec![
        "-hf","unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M",
        "-ngl","99","--cpu-moe","-c","8192","--host","0.0.0.0","--port","8080",
    ]);
}

#[test]
fn builds_cluster_dense_argv() {
    let model = ModelRef::Local("/mnt/ssd/m.gguf".into());
    let eps = vec!["192.168.0.125:50052".to_string(), "192.168.0.83:50052".to_string()];
    let opts = LlamaServerOpts {
        binary: "llama-server", model: &model, rpc_endpoints: &eps,
        ngl: Some(40), tensor_split: Some("0,12,11"), main_gpu: Some(1),
        device: Some("RPC0,RPC1"), cpu_moe: &CpuMoe::Off, ctx: Some(4096),
        host: "0.0.0.0", port: 8080,
    };
    let spec = llama_server_spec(&opts);
    assert_eq!(spec.args, vec![
        "-m","/mnt/ssd/m.gguf",
        "--rpc","192.168.0.125:50052,192.168.0.83:50052",
        "-ngl","40","--tensor-split","0,12,11","--main-gpu","1",
        "--device","RPC0,RPC1","-c","4096","--host","0.0.0.0","--port","8080",
    ]);
}

#[test]
fn builds_n_cpu_moe_argv() {
    let model = ModelRef::Hf("repo:Q4_K_M".into());
    let opts = LlamaServerOpts {
        binary: "llama-server", model: &model, rpc_endpoints: &[],
        ngl: Some(99), tensor_split: None, main_gpu: None, device: None,
        cpu_moe: &CpuMoe::NLayers(32), ctx: None, host: "127.0.0.1", port: 8080,
    };
    let spec = llama_server_spec(&opts);
    assert_eq!(spec.args, vec![
        "-hf","repo:Q4_K_M","-ngl","99","--n-cpu-moe","32","--host","127.0.0.1","--port","8080",
    ]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p airpcez-core builds_moe_solo_argv builds_cluster_dense_argv builds_n_cpu_moe_argv`
Expected: FAIL (types/fn missing).

- [ ] **Step 3: Write minimal implementation**

At the top of `flags.rs` (keep `rpc_server_spec`):
```rust
pub enum ModelRef { Hf(String), Local(String) }
pub enum CpuMoe { Off, All, NLayers(u32) }

pub struct LlamaServerOpts<'a> {
    pub binary: &'a str,
    pub model: &'a ModelRef,
    pub rpc_endpoints: &'a [String],
    pub ngl: Option<u32>,
    pub tensor_split: Option<&'a str>,
    pub main_gpu: Option<u32>,
    pub device: Option<&'a str>,
    pub cpu_moe: &'a CpuMoe,
    pub ctx: Option<u32>,
    pub host: &'a str,
    pub port: u16,
}

pub fn llama_server_spec(opts: &LlamaServerOpts) -> ProcSpec {
    let mut args: Vec<String> = Vec::new();
    match opts.model {
        ModelRef::Hf(v) => { args.push("-hf".into()); args.push(v.clone()); }
        ModelRef::Local(p) => { args.push("-m".into()); args.push(p.clone()); }
    }
    if !opts.rpc_endpoints.is_empty() {
        args.push("--rpc".into());
        args.push(opts.rpc_endpoints.join(","));
    }
    if let Some(n) = opts.ngl { args.push("-ngl".into()); args.push(n.to_string()); }
    if let Some(ts) = opts.tensor_split { args.push("--tensor-split".into()); args.push(ts.into()); }
    if let Some(mg) = opts.main_gpu { args.push("--main-gpu".into()); args.push(mg.to_string()); }
    if let Some(d) = opts.device { args.push("--device".into()); args.push(d.into()); }
    match opts.cpu_moe {
        CpuMoe::Off => {}
        CpuMoe::All => args.push("--cpu-moe".into()),
        CpuMoe::NLayers(n) => { args.push("--n-cpu-moe".into()); args.push(n.to_string()); }
    }
    if let Some(c) = opts.ctx { args.push("-c".into()); args.push(c.to_string()); }
    args.push("--host".into()); args.push(opts.host.into());
    args.push("--port".into()); args.push(opts.port.to_string());
    ProcSpec { program: opts.binary.into(), args }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p airpcez-core builds_moe_solo_argv builds_cluster_dense_argv builds_n_cpu_moe_argv`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/flags.rs
git commit -m "feat(core): llama_server_spec flag-builder (hf/local, rpc, ngl, tensor-split, cpu-moe, ctx)"
```

---

### Task 4: Version-match helper

**Files:**
- Modify: `crates/airpcez-core/src/cluster.rs`

**Interfaces:**
- Consumes: `NodeSnapshot` (Task 1).
- Produces: `fn version_warnings(host_version: Option<&str>, nodes: &[NodeSnapshot]) -> Vec<String>` — one human-readable warning per reachable node whose `binary_version` differs from the host's (the b9789 RPC-compat lesson). Nodes with unknown versions or unreachable nodes are skipped.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `cluster.rs`:
```rust
fn snap(name: &str, ver: Option<&str>, reachable: bool) -> NodeSnapshot {
    use crate::model::Role;
    NodeSnapshot {
        entry: NodeEntry { name: name.into(), addr: format!("{name}:8675") },
        stats: reachable.then(|| NodeStats {
            name: name.into(), role: Role::Worker, ram_total_mib: 1, ram_free_mib: 1,
            cpu_logical: 1, devices: vec![], rpc_endpoint: None,
            binary_version: ver.map(|v| v.into()), running: false, sampled_at_unix: 0,
        }),
        reachable, error: None,
    }
}

#[test]
fn version_warnings_flags_mismatches_only() {
    let nodes = vec![
        snap("a", Some("b9789"), true),   // matches host
        snap("b", Some("b9000"), true),   // mismatch -> warn
        snap("c", None, true),            // unknown -> skip
        snap("d", Some("b9000"), false),  // unreachable -> skip
    ];
    let w = version_warnings(Some("b9789"), &nodes);
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("b") && w[0].contains("b9000") && w[0].contains("b9789"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez-core version_warnings_flags_mismatches_only`
Expected: FAIL (fn missing).

- [ ] **Step 3: Write minimal implementation**

Add to `cluster.rs`:
```rust
/// One warning per reachable node whose binary version differs from the host's.
/// Mismatched llama.cpp versions are the #1 silent RPC failure.
pub fn version_warnings(host_version: Option<&str>, nodes: &[NodeSnapshot]) -> Vec<String> {
    let host = match host_version { Some(h) => h, None => return Vec::new() };
    nodes.iter().filter_map(|n| {
        let v = n.stats.as_ref()?.binary_version.as_deref()?;
        if n.reachable && v != host {
            Some(format!("{} runs llama.cpp {} but host runs {} — RPC may fail", n.entry.name, v, host))
        } else { None }
    }).collect()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez-core version_warnings_flags_mismatches_only`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/cluster.rs
git commit -m "feat(core): version_warnings() for llama.cpp RPC version mismatches"
```

---

### Task 5: Cluster poller

**Files:**
- Modify: `crates/airpcez/Cargo.toml` (promote `reqwest` to a normal dependency)
- Create: `crates/airpcez/src/poller.rs`
- Modify: `crates/airpcez/src/lib.rs` (`pub mod poller;`)

**Interfaces:**
- Consumes: `NodeEntry`, `NodeSnapshot`, `ClusterStatus`, `NodeStats`.
- Produces: `async fn poll_nodes(client: &reqwest::Client, nodes: &[NodeEntry]) -> ClusterStatus` — GET `http://<addr>/stats` per node (2s timeout), building each `NodeSnapshot`.

- [ ] **Step 1: Write the failing integration test**

`crates/airpcez/tests/poller.rs`:
```rust
use airpcez::poller::poll_nodes;
use airpcez_core::cluster::NodeEntry;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn polls_reachable_and_unreachable() {
    // Stand up a real airpcez server serving a mock /stats on one port.
    let stats = NodeStats { name: "up".into(), role: Role::Worker, ram_total_mib: 8,
        ram_free_mib: 4, cpu_logical: 4, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19101, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let nodes = vec![
        NodeEntry { name: "up".into(),   addr: "127.0.0.1:19101".into() },
        NodeEntry { name: "down".into(), addr: "127.0.0.1:1".into() },     // nothing listening
    ];
    let cs = poll_nodes(&reqwest::Client::new(), &nodes).await;
    assert_eq!(cs.nodes.len(), 2);
    assert!(cs.nodes[0].reachable && cs.nodes[0].stats.as_ref().unwrap().name == "up");
    assert!(!cs.nodes[1].reachable && cs.nodes[1].stats.is_none());
}
```
(`AppState::for_test` is added in Task 6 — until then this test won't compile; if implementing Task 5 strictly first, temporarily build `AppState` inline with a `TokioSupervisor`, then switch to `for_test` in Task 6. Recommended: implement Task 5's `poll_nodes` + a minimal inline-AppState test, and let Task 6 introduce `for_test` and update this test.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez polls_reachable_and_unreachable`
Expected: FAIL (`poll_nodes` missing).

- [ ] **Step 3: Write minimal implementation**

In `Cargo.toml`, move `reqwest = { version = "0.12", features = ["json"] }` from `[dev-dependencies]` to `[dependencies]` (keep it in dev too is unnecessary — a normal dep is visible to tests).

`crates/airpcez/src/poller.rs`:
```rust
use airpcez_core::cluster::{ClusterStatus, NodeEntry, NodeSnapshot};
use airpcez_core::model::NodeStats;
use std::time::Duration;

pub async fn poll_nodes(client: &reqwest::Client, nodes: &[NodeEntry]) -> ClusterStatus {
    let mut snapshots = Vec::with_capacity(nodes.len());
    for node in nodes {
        let url = format!("http://{}/stats", node.addr);
        let snap = match client.get(&url).timeout(Duration::from_secs(2)).send().await {
            Ok(resp) => match resp.json::<NodeStats>().await {
                Ok(stats) => NodeSnapshot { entry: node.clone(), stats: Some(stats), reachable: true, error: None },
                Err(e) => NodeSnapshot { entry: node.clone(), stats: None, reachable: true, error: Some(e.to_string()) },
            },
            Err(e) => NodeSnapshot { entry: node.clone(), stats: None, reachable: false, error: Some(e.to_string()) },
        };
        snapshots.push(snap);
    }
    ClusterStatus { nodes: snapshots, warnings: Vec::new() }
}
```
Add `pub mod poller;` to `crates/airpcez/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez polls_reachable_and_unreachable`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/Cargo.toml crates/airpcez/src/poller.rs crates/airpcez/src/lib.rs crates/airpcez/tests/poller.rs
git commit -m "feat: cluster poller (fan out to each node /stats)"
```

---

### Task 6: AppState extension + `GET /cluster`

**Files:**
- Modify: `crates/airpcez/src/server.rs`
- Modify: `crates/airpcez/src/main.rs`
- Modify: `crates/airpcez/tests/stats_endpoint.rs`, `crates/airpcez/tests/ws_stream.rs`, `crates/airpcez/tests/worker_endpoint.rs`, `crates/airpcez/tests/poller.rs` (construct the new AppState)

**Interfaces:**
- Consumes: `poll_nodes`, `NodeEntry`, `ClusterStatus`, `version_warnings`.
- Produces:
  - `AppState` gains `pub nodes: Arc<std::sync::Mutex<Vec<NodeEntry>>>` and `pub http: reqwest::Client`.
  - `AppState::for_test(provider) -> AppState` — convenience for tests (real `TokioSupervisor`, empty nodes, default client).
  - `GET /cluster` → `ClusterStatus` JSON = the host's own snapshot (from `provider.sample()`) **followed by** `poll_nodes(&http, &nodes)`.

- [ ] **Step 1: Write the failing test**

`crates/airpcez/tests/cluster_endpoint.rs`:
```rust
use airpcez_core::cluster::ClusterStatus;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn cluster_endpoint_includes_self() {
    let stats = NodeStats { name: "host".into(), role: Role::Host, ram_total_mib: 16,
        ram_free_mib: 8, cpu_logical: 8, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19102, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let cs: ClusterStatus = reqwest::get("http://127.0.0.1:19102/cluster")
        .await.unwrap().json().await.unwrap();
    assert_eq!(cs.nodes.len(), 1); // just self (no workers configured)
    assert_eq!(cs.nodes[0].entry.name, "host");
    assert!(cs.nodes[0].reachable && cs.nodes[0].stats.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez cluster_endpoint_includes_self`
Expected: FAIL (`for_test`/`/cluster` missing).

- [ ] **Step 3: Write minimal implementation**

In `server.rs`, extend `AppState`:
```rust
#[derive(Clone)]
pub struct AppState {
    pub provider: Arc<dyn StatsProvider>,
    pub supervisor: Arc<dyn ProcessBackend>,
    pub nodes: Arc<std::sync::Mutex<Vec<airpcez_core::cluster::NodeEntry>>>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn for_test(provider: Arc<dyn StatsProvider>) -> AppState {
        AppState {
            provider,
            supervisor: Arc::new(crate::supervisor::TokioSupervisor::new()),
            nodes: Arc::new(std::sync::Mutex::new(Vec::new())),
            http: reqwest::Client::new(),
        }
    }
}
```
Add the route `.route("/cluster", get(cluster_handler))` and:
```rust
async fn cluster_handler(State(s): State<AppState>) -> Json<airpcez_core::cluster::ClusterStatus> {
    use airpcez_core::cluster::*;
    let self_stats = s.provider.sample();
    let self_version = self_stats.binary_version.clone();
    let self_snap = NodeSnapshot {
        entry: NodeEntry { name: self_stats.name.clone(), addr: "self".into() },
        stats: Some(self_stats), reachable: true, error: None,
    };
    let nodes = { s.nodes.lock().unwrap().clone() };
    let mut cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    // Compute version-mismatch warnings against the workers (before inserting self at 0).
    cluster.warnings = version_warnings(self_version.as_deref(), &cluster.nodes);
    cluster.nodes.insert(0, self_snap);
    Json(cluster)
}
```
Update `main.rs` to build the full `AppState` (nodes from `config.nodes`, `http: reqwest::Client::new()`). Update the four existing tests that construct `AppState { provider, supervisor }` to call `AppState::for_test(provider)` instead.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez` (all suites, incl. cluster_endpoint + the updated ones)
Expected: PASS, pristine.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/src/main.rs crates/airpcez/tests/
git commit -m "feat: AppState nodes+http and GET /cluster (self + polled workers)"
```

---

### Task 7: Node add/remove endpoints

**Files:**
- Modify: `crates/airpcez/src/server.rs`

**Interfaces:**
- Consumes: `NodeEntry`, `AppState.nodes`.
- Produces: `POST /nodes` (JSON `NodeEntry` → append if addr not already present) and `DELETE /nodes` (JSON `{ addr: String }` → remove by addr). Both return the updated `Vec<NodeEntry>` as JSON.

- [ ] **Step 1: Write the failing test**

`crates/airpcez/tests/nodes_endpoint.rs`:
```rust
use airpcez_core::cluster::NodeEntry;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn add_then_remove_node() {
    let stats = NodeStats { name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19103, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();
    let added: Vec<NodeEntry> = c.post("http://127.0.0.1:19103/nodes")
        .json(&NodeEntry { name: "w".into(), addr: "192.168.0.9:8675".into() })
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(added.len(), 1);
    let after: Vec<NodeEntry> = c.delete("http://127.0.0.1:19103/nodes")
        .json(&serde_json::json!({"addr":"192.168.0.9:8675"}))
        .send().await.unwrap().json().await.unwrap();
    assert!(after.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez add_then_remove_node`
Expected: FAIL (routes missing).

- [ ] **Step 3: Write minimal implementation**

Add routes `.route("/nodes", post(add_node).delete(remove_node))` and:
```rust
async fn add_node(State(s): State<AppState>, Json(entry): Json<airpcez_core::cluster::NodeEntry>)
    -> Json<Vec<airpcez_core::cluster::NodeEntry>> {
    let mut g = s.nodes.lock().unwrap();
    if !g.iter().any(|n| n.addr == entry.addr) { g.push(entry); }
    Json(g.clone())
}

#[derive(serde::Deserialize)]
struct RemoveNode { addr: String }

async fn remove_node(State(s): State<AppState>, Json(req): Json<RemoveNode>)
    -> Json<Vec<airpcez_core::cluster::NodeEntry>> {
    let mut g = s.nodes.lock().unwrap();
    g.retain(|n| n.addr != req.addr);
    Json(g.clone())
}
```
(Note: persisting the node list back to `airpcez.toml` is deferred to the host launch task wiring; for now the list is in-memory. If you want persistence now, thread the config path into AppState and save on change — but that is optional for this task's test.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez add_then_remove_node`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/tests/nodes_endpoint.rs
git commit -m "feat: POST/DELETE /nodes to manage the worker list"
```

---

### Task 8: Populate `binary_version`

**Files:**
- Create: `crates/airpcez/src/version.rs`
- Modify: `crates/airpcez/src/lib.rs` (`pub mod version;`), `crates/airpcez/src/stats_provider.rs`, `crates/airpcez/src/main.rs`

**Interfaces:**
- Produces:
  - `fn parse_llama_version(stdout: &str) -> Option<String>` — pure: from `llama-server --version` output (`version: 9789 (abc1234)` or `build: 9789 (abc1234)`) → `"b9789"`.
  - `fn detect_binary_version(llama_dir: Option<&str>) -> Option<String>` — runs `<llama_dir>/llama-server --version`, parses it (binary path / process code is fine here — binary crate).
  - `LocalStats` gains `pub llama_dir: Option<String>` and sets `binary_version` via `detect_binary_version`.

- [ ] **Step 1: Write the failing test**

`crates/airpcez/src/version.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_build_number() {
        assert_eq!(parse_llama_version("version: 9789 (abc1234)\nbuilt with ..."), Some("b9789".into()));
        assert_eq!(parse_llama_version("build: 9789 (abc)"), Some("b9789".into()));
        assert_eq!(parse_llama_version("garbage"), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez parses_build_number`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

`crates/airpcez/src/version.rs`:
```rust
/// Parse `llama-server --version` output → "b<N>" (e.g. "b9789").
pub fn parse_llama_version(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let l = line.trim();
        for prefix in ["version:", "build:"] {
            if let Some(rest) = l.strip_prefix(prefix) {
                let num: String = rest.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
                if !num.is_empty() { return Some(format!("b{num}")); }
            }
        }
    }
    None
}

pub fn detect_binary_version(llama_dir: Option<&str>) -> Option<String> {
    let dir = llama_dir?;
    let bin = std::path::Path::new(dir).join("llama-server");
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    // llama.cpp prints --version to stderr on some builds; check both.
    let text = if !out.stdout.is_empty() { String::from_utf8_lossy(&out.stdout).into_owned() }
               else { String::from_utf8_lossy(&out.stderr).into_owned() };
    parse_llama_version(&text)
}
```
Add `pub mod version;` to `lib.rs`. In `stats_provider.rs`, add `pub llama_dir: Option<String>` to `LocalStats`, and in `sample()` set `binary_version: crate::version::detect_binary_version(self.llama_dir.as_deref())`. In `main.rs`, construct `LocalStats { name: config.node_name, role: config.role, llama_dir: config.llama_dir.clone() }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez parses_build_number && cargo build -p airpcez`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/version.rs crates/airpcez/src/lib.rs crates/airpcez/src/stats_provider.rs crates/airpcez/src/main.rs
git commit -m "feat: detect and report llama.cpp binary_version in stats"
```

---

### Task 9: `POST /host/launch` + `/host/stop`

**Files:**
- Modify: `crates/airpcez/src/server.rs`

**Interfaces:**
- Consumes: `llama_server_spec`, `ModelRef`, `CpuMoe`, `poll_nodes`, `AppState`, `Config` (for `llama_dir`, `llama_port`).
- Produces: `POST /host/launch` (request below → builds + supervises `llama-server`, returns `{ openai_url }` or 500) and `POST /host/stop` (`supervisor.stop()`).

Add `pub llama_dir: Option<String>` and `pub llama_port: u16` to `AppState` (populated from config in `main`; `for_test` defaults `llama_dir: None`, `llama_port: 8080`).

Launch request:
```rust
#[derive(serde::Deserialize)]
struct LaunchRequest {
    model_hf: Option<String>,     // exactly one of model_hf/model_path
    model_path: Option<String>,
    ngl: Option<u32>,
    tensor_split: Option<String>,
    main_gpu: Option<u32>,
    device: Option<String>,
    cpu_moe: Option<String>,      // "all" | "off" | a number (n-cpu-moe)
    ctx: Option<u32>,
}
```

- [ ] **Step 1: Write the failing test**

`crates/airpcez/tests/host_launch.rs`:
```rust
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn host_launch_returns_openai_url() {
    let stats = NodeStats { name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None, running: false, sampled_at_unix: 0 };
    let mut state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    state.llama_dir = Some("/bin".into()); // /bin/llama-server won't exist -> override below
    tokio::spawn(airpcez::server::run_server(19104, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // Use a model_hf and rely on the supervisor accepting the spawn attempt; we assert the URL shape.
    let resp = reqwest::Client::new().post("http://127.0.0.1:19104/host/launch")
        .json(&serde_json::json!({"model_hf":"repo:Q4_K_M","ngl":99,"cpu_moe":"all","ctx":4096}))
        .send().await.unwrap();
    // /bin/llama-server doesn't exist -> supervisor.start Err -> 500 (acceptable: proves wiring).
    // If your test box HAS a llama-server on PATH this returns 200 with an openai_url.
    assert!(resp.status() == 500 || resp.status() == 200);
}
```
(This test deliberately tolerates 200/500 because the test box may lack a real `llama-server`; it proves the endpoint builds the spec and drives the supervisor. The argv correctness is already golden-tested in Task 3.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez host_launch_returns_openai_url`
Expected: FAIL (route missing).

- [ ] **Step 3: Write minimal implementation**

Add routes `.route("/host/launch", post(host_launch)).route("/host/stop", post(host_stop))` and:
```rust
async fn host_launch(State(s): State<AppState>, Json(req): Json<LaunchRequest>) -> impl IntoResponse {
    use airpcez_core::flags::*;
    let model = match (req.model_hf, req.model_path) {
        (Some(hf), _) => ModelRef::Hf(hf),
        (None, Some(p)) => ModelRef::Local(p),
        (None, None) => return (StatusCode::BAD_REQUEST, "model_hf or model_path required".to_string()).into_response(),
    };
    let cpu_moe = match req.cpu_moe.as_deref() {
        Some("all") => CpuMoe::All,
        Some("off") | None => CpuMoe::Off,
        Some(n) => match n.parse::<u32>() { Ok(v) => CpuMoe::NLayers(v), Err(_) => CpuMoe::Off },
    };
    // Gather worker rpc endpoints from the polled cluster.
    let nodes = { s.nodes.lock().unwrap().clone() };
    let cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    let eps: Vec<String> = cluster.nodes.iter()
        .filter_map(|n| n.stats.as_ref().and_then(|st| st.rpc_endpoint.clone()))
        .collect();
    let binary = match &s.llama_dir {
        Some(d) => format!("{d}/llama-server"),
        None => "llama-server".to_string(),
    };
    let opts = LlamaServerOpts {
        binary: &binary, model: &model, rpc_endpoints: &eps,
        ngl: req.ngl, tensor_split: req.tensor_split.as_deref(), main_gpu: req.main_gpu,
        device: req.device.as_deref(), cpu_moe: &cpu_moe, ctx: req.ctx,
        host: "0.0.0.0", port: s.llama_port,
    };
    match s.supervisor.start(llama_server_spec(&opts)) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({
            "openai_url": format!("http://localhost:{}/v1", s.llama_port)
        }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn host_stop(State(s): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "stopped": s.supervisor.stop() })))
}
```
Add the two `AppState` fields and update `for_test` + `main`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez` (all suites)
Expected: PASS, pristine.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/src/main.rs crates/airpcez/tests/host_launch.rs
git commit -m "feat: POST /host/launch (build+supervise llama-server) and /host/stop"
```

---

### Task 10: Host cockpit UI

**Files:**
- Modify: `crates/airpcez/assets/index.html`

**Interfaces:**
- Consumes: `GET /cluster`, `POST /nodes`, `DELETE /nodes`, `POST /host/launch`, `POST /host/stop` (and the existing `/ws`, `/worker/start|stop`).

This task is verified manually (UI). Extend the layout-A page:
- **Left "Cluster" panel:** every ~2s `fetch('/cluster')` → render each node row: name, role, reachable dot, RAM bar (`ram_free_mib`/`ram_total_mib`), VRAM bar (first device), a `⚠ unreliable` chip when `devices[0].reliable === false`, and the `binary_version`. Show a banner listing `cs.warnings` (the server-computed version-mismatch warnings) when non-empty. An "Add node" mini-form (name + `host:8675` addr) POSTs `/nodes`; each row has a remove (✕) that DELETEs `/nodes`.
- **Right "Run a Model" panel:** inputs for model (`-hf repo:quant` text OR a local path), `-ngl`, `--tensor-split`, `--main-gpu`, `--device`, a `--cpu-moe` selector (`off` / `all` / a number for `--n-cpu-moe`), and `-c` context. A **Launch** button POSTs `/host/launch` and shows the returned `openai_url` (with a link to `http://<host>:<llama_port>` for llama-server's built-in chat) or the error. A **Stop** button POSTs `/host/stop`.
- Keep the existing worker Start/Stop controls (a machine can still be a worker).

- [ ] **Step 1: Implement the UI**

Write the HTML/JS per above. Keep it plain (inline `<style>`/`<script>`, no framework), readable, layout A. Use the exact field names from `NodeStats`/`ClusterStatus`.

- [ ] **Step 2: Manual smoke**

Run: `cargo run -p airpcez` → open `http://localhost:8675`. Confirm: the Cluster panel shows this host (self) live; adding a node address makes a row appear (unreachable is fine without a second instance); the Run-a-Model form posts to `/host/launch` (a 500 is expected with no real `llama-server`/`llama_dir`, but the request shape must be correct); no console errors. Stop the server.

- [ ] **Step 3: Commit**

```bash
git add crates/airpcez/assets/index.html
git commit -m "feat: host cockpit UI (cluster panel, node mgmt, run-a-model, serving info)"
```

---

## Phase 2 Definition of Done

`cargo test` green and pristine. Running an airpcez instance as host: the cockpit shows the whole cluster live (self + polled workers, with version-mismatch warnings and bogus-VRAM flags), you can add/remove worker nodes, and **Run a Model** builds the correct `llama-server` argv (`-hf`/`-m`, `--rpc <workers>`, `-ngl`, `--tensor-split`, `--cpu-moe`/`--n-cpu-moe`, `-c`) and launches it, surfacing the OpenAI URL. Sets up Phase 3 (the suggestion/pre-flight engine plugs into the launch form).

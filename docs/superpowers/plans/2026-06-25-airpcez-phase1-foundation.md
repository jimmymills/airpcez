# airpcez Phase 1 — Foundation + Worker Node — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A runnable `airpcez` binary that serves a local web UI showing this machine's live RAM/VRAM/CPU and can start/stop the machine as a llama.cpp RPC worker.

**Architecture:** A Cargo workspace with a platform-agnostic `airpcez-core` library (types + traits + pure logic) and an `airpcez` binary (axum HTTP/WS server, OS-specific stats/process backends, embedded web UI). All OS-specific code sits behind `airpcez-core` traits so the core stays portable.

**Tech Stack:** Rust 2021 (builds on stable), `tokio`, `axum` (HTTP + WebSocket), `serde`/`serde_json`, `toml`, `sysinfo` (RAM/CPU), `nvidia-smi` parsing (NVIDIA VRAM), `sysctl`/`vm_stat`/`system_profiler` parsing (macOS RAM + Metal).

## Global Constraints

- Rust edition 2021; must build on stable toolchain. One line each, applied to every task.
- `airpcez-core` MUST contain no OS-specific code (no `std::process`, no `Command`, no `#[cfg(target_os)]`). OS code lives in the `airpcez` binary behind core traits.
- Wraps llama.cpp release **b9789** binaries (`rpc-server`, `llama-server`) at a configured path; never reimplements inference.
- Default ports (configurable): airpcez UI `8675`, llama.cpp RPC `50052`, `llama-server` `8080`.
- v1 platforms: macOS (Apple Silicon) and Linux + NVIDIA. No heavy SPA framework — embedded static HTML/JS.
- A VRAM reading that overflows or exceeds physical total is **unreliable** and must be flagged, never trusted (the Vulkan `17592186044362 MiB` lesson).

---

### Task 1: Cargo workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/airpcez-core/Cargo.toml`
- Create: `crates/airpcez-core/src/lib.rs`
- Create: `crates/airpcez/Cargo.toml`
- Create: `crates/airpcez/src/main.rs`

**Interfaces:**
- Produces: a buildable workspace; `airpcez_core` crate compiles and is depended on by `airpcez`.

- [ ] **Step 1: Write workspace + core manifests and a trivial failing test**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/airpcez-core", "crates/airpcez"]
```

`crates/airpcez-core/Cargo.toml`:
```toml
[package]
name = "airpcez-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

`crates/airpcez-core/src/lib.rs`:
```rust
#[cfg(test)]
mod tests {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
```

`crates/airpcez/Cargo.toml`:
```toml
[package]
name = "airpcez"
version = "0.1.0"
edition = "2021"

[dependencies]
airpcez-core = { path = "../airpcez-core" }
tokio = { version = "1", features = ["full"] }
axum = { version = "0.7", features = ["ws"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
sysinfo = "0.31"
```

`crates/airpcez/src/main.rs`:
```rust
fn main() {
    println!("airpcez");
}
```

- [ ] **Step 2: Run tests to verify the workspace builds**

Run: `cargo test`
Expected: PASS (`workspace_builds`), `cargo build` succeeds for both crates.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml crates/
git commit -m "feat: cargo workspace scaffold (airpcez-core + airpcez)"
```

---

### Task 2: Core cluster model types

**Files:**
- Create: `crates/airpcez-core/src/model.rs`
- Modify: `crates/airpcez-core/src/lib.rs` (add `pub mod model;`)

**Interfaces:**
- Produces:
  - `enum Role { Worker, Host }`
  - `struct DeviceStats { name: String, kind: DeviceKind, vram_total_mib: u64, vram_free_mib: u64, reliable: bool }`
  - `enum DeviceKind { Cuda, Metal, Cpu, Other }`
  - `struct NodeStats { name: String, role: Role, ram_total_mib: u64, ram_free_mib: u64, cpu_logical: u32, devices: Vec<DeviceStats>, rpc_endpoint: Option<String>, binary_version: Option<String>, running: bool, sampled_at_unix: u64 }`
  - All `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]`.

- [ ] **Step 1: Write the failing test**

`crates/airpcez-core/src/model.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_stats_json_roundtrips() {
        let s = NodeStats {
            name: "mac-host".into(),
            role: Role::Worker,
            ram_total_mib: 16384,
            ram_free_mib: 10240,
            cpu_logical: 12,
            devices: vec![DeviceStats {
                name: "MTL0".into(),
                kind: DeviceKind::Metal,
                vram_total_mib: 12288,
                vram_free_mib: 11000,
                reliable: true,
            }],
            rpc_endpoint: Some("192.168.0.125:50052".into()),
            binary_version: Some("b9789".into()),
            running: false,
            sampled_at_unix: 1782415690,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: NodeStats = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez-core node_stats_json_roundtrips`
Expected: FAIL (types not defined / module missing).

- [ ] **Step 3: Write minimal implementation**

At the top of `crates/airpcez-core/src/model.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role { Worker, Host }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind { Cuda, Metal, Cpu, Other }

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct DeviceStats {
    pub name: String,
    pub kind: DeviceKind,
    pub vram_total_mib: u64,
    pub vram_free_mib: u64,
    pub reliable: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct NodeStats {
    pub name: String,
    pub role: Role,
    pub ram_total_mib: u64,
    pub ram_free_mib: u64,
    pub cpu_logical: u32,
    pub devices: Vec<DeviceStats>,
    pub rpc_endpoint: Option<String>,
    pub binary_version: Option<String>,
    pub running: bool,
    pub sampled_at_unix: u64,
}
```

Add `pub mod model;` to `crates/airpcez-core/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez-core node_stats_json_roundtrips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/
git commit -m "feat(core): cluster model types (NodeStats, DeviceStats, Role)"
```

---

### Task 3: VRAM reliability rule (the Vulkan-overflow lesson)

**Files:**
- Modify: `crates/airpcez-core/src/model.rs`

**Interfaces:**
- Produces: `fn vram_reliable(total_mib: u64, free_mib: u64) -> bool` — a pure rule reused by every stats backend.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `model.rs`:
```rust
#[test]
fn flags_overflow_and_over_physical_vram_as_unreliable() {
    // Real 2080 Super: 8 GB total, sane free.
    assert!(vram_reliable(8192, 7700));
    // The Vulkan overflow we hit: ~16 EB "free".
    assert!(!vram_reliable(8192, 17_592_186_044_362));
    // Free exceeding total is impossible -> unreliable.
    assert!(!vram_reliable(8192, 9000));
    // Zero total (no real device) -> unreliable.
    assert!(!vram_reliable(0, 0));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez-core flags_overflow`
Expected: FAIL (`vram_reliable` not found).

- [ ] **Step 3: Write minimal implementation**

Add to `model.rs`:
```rust
/// A VRAM reading is trustworthy only if total is non-zero and free <= total.
/// Drivers (e.g. some Vulkan setups) can report an overflowed "free" value far
/// larger than physical memory; treat any such reading as unreliable.
pub fn vram_reliable(total_mib: u64, free_mib: u64) -> bool {
    total_mib > 0 && free_mib <= total_mib
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez-core flags_overflow`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/model.rs
git commit -m "feat(core): vram_reliable() rule for bogus VRAM detection"
```

---

### Task 4: StatsProvider trait + mock

**Files:**
- Create: `crates/airpcez-core/src/stats.rs`
- Modify: `crates/airpcez-core/src/lib.rs` (add `pub mod stats;`)

**Interfaces:**
- Consumes: `NodeStats` (Task 2).
- Produces:
  - `trait StatsProvider { fn sample(&self) -> NodeStats; }`
  - `struct MockStatsProvider { pub stats: NodeStats }` implementing it (test/dev double).

- [ ] **Step 1: Write the failing test**

`crates/airpcez-core/src/stats.rs`:
```rust
use crate::model::*;

pub trait StatsProvider: Send + Sync {
    fn sample(&self) -> NodeStats;
}

pub struct MockStatsProvider {
    pub stats: NodeStats,
}

impl StatsProvider for MockStatsProvider {
    fn sample(&self) -> NodeStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mock_provider_returns_its_stats() {
        let stats = NodeStats {
            name: "n".into(), role: Role::Worker, ram_total_mib: 8, ram_free_mib: 4,
            cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
            running: false, sampled_at_unix: 0,
        };
        let p = MockStatsProvider { stats: stats.clone() };
        assert_eq!(p.sample(), stats);
    }
}
```

Add `pub mod stats;` to `lib.rs`.

- [ ] **Step 2: Run test to verify it fails, then passes**

Run: `cargo test -p airpcez-core mock_provider`
Expected: FAIL before adding the module, PASS after (the code above is the implementation — confirm it compiles and passes).

- [ ] **Step 3: Commit**

```bash
git add crates/airpcez-core/src/
git commit -m "feat(core): StatsProvider trait + MockStatsProvider"
```

---

### Task 5: NVIDIA VRAM parser (pure)

**Files:**
- Create: `crates/airpcez/src/stats_nvidia.rs`
- Create: `crates/airpcez/tests/fixtures/nvidia-smi.csv`

**Interfaces:**
- Consumes: `DeviceStats`, `DeviceKind`, `vram_reliable` (core).
- Produces: `fn parse_nvidia_smi(csv: &str) -> Vec<DeviceStats>` — parses the no-header CSV from `nvidia-smi --query-gpu=name,memory.total,memory.free --format=csv,noheader,nounits`.

- [ ] **Step 1: Write the fixture and failing test**

`crates/airpcez/tests/fixtures/nvidia-smi.csv` (captured real output, units = MiB):
```
NVIDIA GeForce RTX 2080 SUPER, 8192, 7700
```

`crates/airpcez/src/stats_nvidia.rs`:
```rust
use airpcez_core::model::{DeviceKind, DeviceStats, vram_reliable};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_one_gpu() {
        let csv = include_str!("../tests/fixtures/nvidia-smi.csv");
        let devs = parse_nvidia_smi(csv);
        assert_eq!(devs.len(), 1);
        let d = &devs[0];
        assert_eq!(d.name, "NVIDIA GeForce RTX 2080 SUPER");
        assert_eq!(d.kind, DeviceKind::Cuda);
        assert_eq!(d.vram_total_mib, 8192);
        assert_eq!(d.vram_free_mib, 7700);
        assert!(d.reliable);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez parses_one_gpu`
Expected: FAIL (`parse_nvidia_smi` not defined).

- [ ] **Step 3: Write minimal implementation**

Add to `stats_nvidia.rs`:
```rust
pub fn parse_nvidia_smi(csv: &str) -> Vec<DeviceStats> {
    csv.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            if cols.len() < 3 { return None; }
            let total: u64 = cols[1].parse().ok()?;
            let free: u64 = cols[2].parse().ok()?;
            Some(DeviceStats {
                name: cols[0].to_string(),
                kind: DeviceKind::Cuda,
                vram_total_mib: total,
                vram_free_mib: free,
                reliable: vram_reliable(total, free),
            })
        })
        .collect()
}
```

Wire the module in `crates/airpcez/src/main.rs`: add `mod stats_nvidia;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez parses_one_gpu`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/stats_nvidia.rs crates/airpcez/tests/fixtures/nvidia-smi.csv crates/airpcez/src/main.rs
git commit -m "feat: parse nvidia-smi VRAM output with reliability flag"
```

---

### Task 6: macOS memory parsers (pure)

**Files:**
- Create: `crates/airpcez/src/stats_macos.rs`
- Create: `crates/airpcez/tests/fixtures/vm_stat.txt`

**Interfaces:**
- Produces:
  - `fn parse_vm_stat_free_mib(vm_stat: &str, page_size_bytes: u64) -> u64` — sums free+inactive+speculative pages → MiB of genuinely reclaimable RAM (separating real-free from Metal working set, per the lesson).
  - `fn parse_metal_vram_mib(system_profiler: &str) -> Option<u64>` — extracts the Metal "recommendedMaxWorkingSetSize"-style total from `system_profiler SPDisplaysDataType` (returns total; free is approximated from system RAM by the caller).

- [ ] **Step 1: Write the fixture and failing test**

`crates/airpcez/tests/fixtures/vm_stat.txt` (captured, 16 KB pages):
```
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                                   50000.
Pages active:                                244695.
Pages inactive:                              120000.
Pages speculative:                            10000.
Pages wired down:                            120927.
```

`crates/airpcez/src/stats_macos.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn computes_real_free_mib_from_vm_stat() {
        let txt = include_str!("../tests/fixtures/vm_stat.txt");
        // free(50000) + inactive(120000) + speculative(10000) = 180000 pages
        // * 16384 bytes / 1MiB = 2812.5 MiB -> 2812
        let mib = parse_vm_stat_free_mib(txt, 16384);
        assert_eq!(mib, 2812);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez computes_real_free_mib`
Expected: FAIL (function not defined).

- [ ] **Step 3: Write minimal implementation**

Add to `stats_macos.rs`:
```rust
fn pages_named(vm_stat: &str, label: &str) -> u64 {
    vm_stat.lines()
        .find(|l| l.trim_start().starts_with(label))
        .and_then(|l| l.rsplit(|c: char| c == ' ' || c == ':')
            .map(|t| t.trim().trim_end_matches('.'))
            .find(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Reclaimable RAM = free + inactive + speculative pages. Excludes wired
/// (e.g. Metal GPU buffers) so we report what a launch can actually use.
pub fn parse_vm_stat_free_mib(vm_stat: &str, page_size_bytes: u64) -> u64 {
    let pages = pages_named(vm_stat, "Pages free")
        + pages_named(vm_stat, "Pages inactive")
        + pages_named(vm_stat, "Pages speculative");
    pages * page_size_bytes / (1024 * 1024)
}

pub fn parse_metal_vram_mib(system_profiler: &str) -> Option<u64> {
    // Look for a "VRAM (Total): N GB" or "VRAM (Dynamic, Max): N MB" line.
    for line in system_profiler.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("VRAM").and_then(|r| r.split_once(':').map(|x| x.1)) {
            let rest = rest.trim();
            let (num, unit) = rest.split_once(' ')?;
            let n: f64 = num.trim().parse().ok()?;
            let mib = match unit.trim() { "GB" => n * 1024.0, _ => n };
            return Some(mib as u64);
        }
    }
    None
}
```

Wire `mod stats_macos;` in `main.rs` behind `#[cfg(target_os = "macos")]`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez computes_real_free_mib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/stats_macos.rs crates/airpcez/tests/fixtures/vm_stat.txt crates/airpcez/src/main.rs
git commit -m "feat: macOS real-free RAM + Metal VRAM parsers"
```

---

### Task 7: Platform StatsProvider implementation

**Files:**
- Create: `crates/airpcez/src/stats_provider.rs`
- Modify: `crates/airpcez/src/main.rs` (`mod stats_provider;`)

**Interfaces:**
- Consumes: `StatsProvider` trait, `parse_nvidia_smi`, macOS parsers, `sysinfo`.
- Produces: `struct LocalStats { name: String, role: Role }` impl `StatsProvider` — gathers RAM/CPU via `sysinfo`, GPU via the platform path, returns `NodeStats`. Spawning `nvidia-smi`/`system_profiler` happens here (binary, not core).

- [ ] **Step 1: Write the implementation (integration-tested by Task 8)**

`crates/airpcez/src/stats_provider.rs`:
```rust
use airpcez_core::model::*;
use airpcez_core::stats::StatsProvider;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LocalStats { pub name: String, pub role: Role }

impl StatsProvider for LocalStats {
    fn sample(&self) -> NodeStats {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let ram_total_mib = sys.total_memory() / (1024 * 1024);
        let ram_free_mib = sys.available_memory() / (1024 * 1024);
        let cpu_logical = num_cpus_logical();
        let devices = gather_devices();
        NodeStats {
            name: self.name.clone(),
            role: self.role,
            ram_total_mib,
            ram_free_mib,
            cpu_logical,
            devices,
            rpc_endpoint: None,
            binary_version: None,
            running: false,
            sampled_at_unix: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        }
    }
}

fn num_cpus_logical() -> u32 {
    std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1)
}

#[cfg(target_os = "linux")]
fn gather_devices() -> Vec<DeviceStats> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total,memory.free", "--format=csv,noheader,nounits"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            crate::stats_nvidia::parse_nvidia_smi(&String::from_utf8_lossy(&o.stdout))
        }
        _ => vec![],
    }
}

#[cfg(target_os = "macos")]
fn gather_devices() -> Vec<DeviceStats> {
    let sp = Command::new("system_profiler").arg("SPDisplaysDataType").output();
    let total = sp.ok()
        .filter(|o| o.status.success())
        .and_then(|o| crate::stats_macos::parse_metal_vram_mib(&String::from_utf8_lossy(&o.stdout)));
    // Apple unified memory: approximate VRAM free from system RAM real-free.
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let free = sys.available_memory() / (1024 * 1024);
    match total {
        Some(t) => vec![DeviceStats {
            name: "MTL0".into(), kind: DeviceKind::Metal,
            vram_total_mib: t, vram_free_mib: free.min(t),
            reliable: vram_reliable(t, free.min(t)),
        }],
        None => vec![],
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn gather_devices() -> Vec<DeviceStats> { vec![] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p airpcez`
Expected: builds on the current OS.

- [ ] **Step 3: Commit**

```bash
git add crates/airpcez/src/stats_provider.rs crates/airpcez/src/main.rs
git commit -m "feat: LocalStats StatsProvider (sysinfo + GPU per platform)"
```

---

### Task 8: axum server + `/stats` endpoint

**Files:**
- Create: `crates/airpcez/src/server.rs`
- Modify: `crates/airpcez/src/main.rs` (start the server)

**Interfaces:**
- Consumes: `LocalStats`, `StatsProvider`.
- Produces: `async fn run_server(port: u16, provider: Arc<dyn StatsProvider>)`; HTTP `GET /stats` → `NodeStats` JSON.

- [ ] **Step 1: Write the failing integration test**

`crates/airpcez/tests/stats_endpoint.rs`:
```rust
use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use std::sync::Arc;

#[tokio::test]
async fn stats_endpoint_returns_node_stats() {
    let stats = NodeStats {
        name: "test".into(), role: Role::Worker, ram_total_mib: 16, ram_free_mib: 8,
        cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let provider = Arc::new(MockStatsProvider { stats: stats.clone() });
    tokio::spawn(airpcez::server::run_server(18675, provider));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let got: NodeStats = reqwest::get("http://127.0.0.1:18675/stats")
        .await.unwrap().json().await.unwrap();
    assert_eq!(got, stats);
}
```

Add `[dev-dependencies] reqwest = { version = "0.12", features = ["json"] }` to `crates/airpcez/Cargo.toml`, and expose the lib: create `crates/airpcez/src/lib.rs` with `pub mod server;` and add `[lib]`/`[[bin]]` targets if needed (or keep `server` public via `pub mod server;` in `main.rs` is insufficient for the test — add a `lib.rs` exposing `pub mod server;` and re-export from `main.rs`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez stats_endpoint_returns_node_stats`
Expected: FAIL (`run_server` not defined).

- [ ] **Step 3: Write minimal implementation**

`crates/airpcez/src/server.rs`:
```rust
use airpcez_core::stats::StatsProvider;
use axum::{routing::get, Json, Router, extract::State};
use std::sync::Arc;

type Provider = Arc<dyn StatsProvider>;

pub async fn run_server(port: u16, provider: Provider) {
    let app = Router::new()
        .route("/stats", get(stats_handler))
        .with_state(provider);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn stats_handler(State(p): State<Provider>) -> Json<airpcez_core::model::NodeStats> {
    Json(p.sample())
}
```

Create `crates/airpcez/src/lib.rs` with `pub mod server;` (plus `pub mod stats_nvidia;` etc. as needed) and have `main.rs` call `airpcez::server::run_server`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez stats_endpoint_returns_node_stats`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/
git commit -m "feat: axum server with GET /stats"
```

---

### Task 9: WebSocket live-stats stream

**Files:**
- Modify: `crates/airpcez/src/server.rs`

**Interfaces:**
- Produces: `GET /ws` upgrade → pushes a `NodeStats` JSON frame every 1s.

- [ ] **Step 1: Write the failing test**

`crates/airpcez/tests/ws_stream.rs`:
```rust
use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio_tungstenite::connect_async;

#[tokio::test]
async fn ws_pushes_stats_frames() {
    let stats = NodeStats { name: "ws".into(), role: Role::Worker, ram_total_mib: 1,
        ram_free_mib: 1, cpu_logical: 1, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let provider = Arc::new(MockStatsProvider { stats: stats.clone() });
    tokio::spawn(airpcez::server::run_server(18676, provider));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (mut ws, _) = connect_async("ws://127.0.0.1:18676/ws").await.unwrap();
    let msg = ws.next().await.unwrap().unwrap();
    let got: NodeStats = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(got.name, "ws");
}
```

Add dev-deps: `futures-util = "0.3"`, `tokio-tungstenite = "0.23"`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez ws_pushes_stats_frames`
Expected: FAIL (no `/ws` route).

- [ ] **Step 3: Write minimal implementation**

In `server.rs`, add the route `.route("/ws", get(ws_handler))` and:
```rust
use axum::extract::ws::{WebSocket, WebSocketUpgrade, Message};
use std::time::Duration;

async fn ws_handler(ws: WebSocketUpgrade, State(p): State<Provider>) -> axum::response::Response {
    ws.on_upgrade(move |socket| ws_loop(socket, p))
}

async fn ws_loop(mut socket: WebSocket, p: Provider) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let json = serde_json::to_string(&p.sample()).unwrap();
        if socket.send(Message::Text(json)).await.is_err() { break; }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez ws_pushes_stats_frames`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/
git commit -m "feat: WebSocket /ws live stats stream"
```

---

### Task 10: ProcessBackend trait + tokio supervisor

**Files:**
- Create: `crates/airpcez-core/src/process.rs` (trait only — core)
- Modify: `crates/airpcez-core/src/lib.rs` (`pub mod process;`)
- Create: `crates/airpcez/src/supervisor.rs` (tokio impl — binary)

**Interfaces:**
- Produces (core):
  - `struct ProcSpec { program: String, args: Vec<String> }`
  - `enum ProcStatus { Stopped, Running, Exited(i32), Crashed(String) }`
  - `trait ProcessBackend: Send + Sync { fn start(&self, spec: ProcSpec) -> Result<(), String>; fn stop(&self); fn status(&self) -> ProcStatus; fn recent_logs(&self) -> Vec<String>; }`
- Produces (binary): `struct TokioSupervisor` implementing `ProcessBackend`.

- [ ] **Step 1: Write the core trait + a failing supervisor test**

`crates/airpcez-core/src/process.rs`:
```rust
#[derive(Clone, Debug, PartialEq)]
pub struct ProcSpec { pub program: String, pub args: Vec<String> }

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ProcStatus { #[default] Stopped, Running, Exited(i32), Crashed(String) }

pub trait ProcessBackend: Send + Sync {
    fn start(&self, spec: ProcSpec) -> Result<(), String>;
    fn stop(&self);
    fn status(&self) -> ProcStatus;
    fn recent_logs(&self) -> Vec<String>;
}
```
Add `pub mod process;` to core `lib.rs`.

`crates/airpcez/tests/supervisor.rs`:
```rust
use airpcez_core::process::*;
use airpcez::supervisor::TokioSupervisor;

#[tokio::test]
async fn runs_and_captures_output() {
    let sup = TokioSupervisor::new();
    sup.start(ProcSpec { program: "echo".into(), args: vec!["hello-airpcez".into()] }).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(matches!(sup.status(), ProcStatus::Exited(0)));
    assert!(sup.recent_logs().iter().any(|l| l.contains("hello-airpcez")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez runs_and_captures_output`
Expected: FAIL (`TokioSupervisor` not defined).

- [ ] **Step 3: Write minimal implementation**

`crates/airpcez/src/supervisor.rs`:
```rust
use airpcez_core::process::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Default)]
struct Inner { status: ProcStatus, logs: Vec<String>, child: Option<tokio::process::Child> }

pub struct TokioSupervisor { inner: Arc<Mutex<Inner>> }

impl TokioSupervisor {
    pub fn new() -> Self { Self { inner: Arc::new(Mutex::new(Inner::default())) } }
}

impl ProcessBackend for TokioSupervisor {
    fn start(&self, spec: ProcSpec) -> Result<(), String> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let inner = self.inner.clone();
        { let mut g = inner.lock().unwrap(); g.status = ProcStatus::Running; }
        let li = inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut g = li.lock().unwrap();
                g.logs.push(l);
                if g.logs.len() > 500 { g.logs.remove(0); }
            }
        });
        let le = inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut g = le.lock().unwrap();
                g.logs.push(l);
                if g.logs.len() > 500 { g.logs.remove(0); }
            }
        });
        let lw = inner.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            let mut g = lw.lock().unwrap();
            g.status = match status {
                Ok(s) if s.success() => ProcStatus::Exited(0),
                Ok(s) => ProcStatus::Exited(s.code().unwrap_or(-1)),
                Err(e) => ProcStatus::Crashed(e.to_string()),
            };
        });
        Ok(())
    }
    fn stop(&self) {
        let mut g = self.inner.lock().unwrap();
        if let Some(child) = g.child.as_mut() { let _ = child.start_kill(); }
        g.status = ProcStatus::Stopped;
    }
    fn status(&self) -> ProcStatus { self.inner.lock().unwrap().status.clone() }
    fn recent_logs(&self) -> Vec<String> { self.inner.lock().unwrap().logs.clone() }
}
```
Expose `pub mod supervisor;` in the binary `lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p airpcez runs_and_captures_output`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/process.rs crates/airpcez-core/src/lib.rs crates/airpcez/src/supervisor.rs crates/airpcez/src/lib.rs
git commit -m "feat: ProcessBackend trait + TokioSupervisor"
```

---

### Task 11: rpc-server argv builder + worker start/stop endpoints

**Files:**
- Create: `crates/airpcez-core/src/flags.rs` (pure builder — core)
- Modify: `crates/airpcez-core/src/lib.rs`
- Modify: `crates/airpcez/src/server.rs` (POST `/worker/start`, `/worker/stop`)

**Interfaces:**
- Consumes: `ProcSpec`, `DeviceStats`.
- Produces (core): `fn rpc_server_spec(binary: &str, host: &str, port: u16, device: Option<&str>) -> ProcSpec`.

- [ ] **Step 1: Write the failing test**

Add to `crates/airpcez-core/src/flags.rs`:
```rust
use crate::process::ProcSpec;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_rpc_server_argv_with_device_pin() {
        let spec = rpc_server_spec("/opt/llama/rpc-server", "0.0.0.0", 50052, Some("MTL0"));
        assert_eq!(spec.program, "/opt/llama/rpc-server");
        assert_eq!(spec.args, vec!["-H","0.0.0.0","-p","50052","-d","MTL0"]);
    }
    #[test]
    fn omits_device_flag_when_none() {
        let spec = rpc_server_spec("rpc-server", "0.0.0.0", 50052, None);
        assert_eq!(spec.args, vec!["-H","0.0.0.0","-p","50052"]);
    }
}
```
Add `pub mod flags;` to core `lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez-core builds_rpc_server_argv`
Expected: FAIL.

- [ ] **Step 3: Write minimal implementation**

In `flags.rs`:
```rust
pub fn rpc_server_spec(binary: &str, host: &str, port: u16, device: Option<&str>) -> ProcSpec {
    let mut args = vec!["-H".into(), host.into(), "-p".into(), port.to_string()];
    if let Some(d) = device { args.push("-d".into()); args.push(d.into()); }
    ProcSpec { program: binary.into(), args }
}
```

Then in `server.rs` add `POST /worker/start` (reads configured binary path + rpc port + pinned device, calls `rpc_server_spec`, hands to the supervisor) and `POST /worker/stop` (calls `supervisor.stop()`). Add an integration test `crates/airpcez/tests/worker_endpoint.rs` that POSTs `/worker/start` with `program:"echo"` override and asserts a 200 + status flips to running/exited.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p airpcez-core builds_rpc_server_argv && cargo test -p airpcez worker`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez-core/src/flags.rs crates/airpcez-core/src/lib.rs crates/airpcez/src/server.rs crates/airpcez/tests/worker_endpoint.rs
git commit -m "feat: rpc-server argv builder + /worker start/stop"
```

---

### Task 12: Config (TOML) + embedded web UI (layout A skeleton)

**Files:**
- Create: `crates/airpcez/src/config.rs`
- Create: `crates/airpcez/assets/index.html`
- Modify: `crates/airpcez/src/server.rs` (serve `GET /` → embedded HTML)
- Modify: `crates/airpcez/src/main.rs` (load config, wire ports/role)

**Interfaces:**
- Produces: `struct Config { ui_port: u16, rpc_port: u16, llama_port: u16, role: Role, llama_dir: Option<String>, node_name: String }` with `load(path) -> Config` (defaults if missing) and `save(path)`.

- [ ] **Step 1: Write the failing config test**

`crates/airpcez/tests/config.rs`:
```rust
use airpcez::config::Config;
#[test]
fn defaults_when_missing_then_roundtrips() {
    let dir = std::env::temp_dir().join(format!("airpcez-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let c = Config::load(&path);
    assert_eq!(c.ui_port, 8675);
    assert_eq!(c.rpc_port, 50052);
    assert_eq!(c.llama_port, 8080);
    c.save(&path).unwrap();
    let c2 = Config::load(&path);
    assert_eq!(c.ui_port, c2.ui_port);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p airpcez defaults_when_missing`
Expected: FAIL.

- [ ] **Step 3: Write config + serve the UI**

`crates/airpcez/src/config.rs`:
```rust
use airpcez_core::model::Role;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub ui_port: u16,
    pub rpc_port: u16,
    pub llama_port: u16,
    pub role: Role,
    pub llama_dir: Option<String>,
    pub node_name: String,
}
impl Default for Config {
    fn default() -> Self {
        Self { ui_port: 8675, rpc_port: 50052, llama_port: 8080, role: Role::Worker,
            llama_dir: None,
            node_name: hostname::get().ok().and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "airpcez-node".into()) }
    }
}
impl Config {
    pub fn load(path: &Path) -> Config {
        std::fs::read_to_string(path).ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, toml::to_string_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
}
```
Add `hostname = "0.4"` to deps.

`crates/airpcez/assets/index.html` — layout A skeleton: a left node panel (this machine's name + two `<progress>`-style RAM/VRAM bars updated from `/ws`), and a right workspace with a Worker Start/Stop button calling `/worker/start|stop`. Plain HTML + a small `<script>` opening `new WebSocket("ws://"+location.host+"/ws")` and updating the bars.

In `server.rs`, add `.route("/", get(|| async { axum::response::Html(include_str!("../assets/index.html")) }))`.

- [ ] **Step 4: Run test + manual smoke**

Run: `cargo test -p airpcez defaults_when_missing`
Expected: PASS.
Manual: `cargo run -p airpcez`, open `http://localhost:8675` → see live RAM/VRAM bars; Worker Start/Stop toggles `rpc-server`.

- [ ] **Step 5: Commit**

```bash
git add crates/airpcez/src/config.rs crates/airpcez/assets/index.html crates/airpcez/src/server.rs crates/airpcez/src/main.rs crates/airpcez/Cargo.toml
git commit -m "feat: TOML config + embedded web UI (layout A skeleton)"
```

---

## Phase 1 Definition of Done

`cargo test` green; running `airpcez` on a Mac or Linux+NVIDIA box serves `http://localhost:8675` showing this machine's live RAM/VRAM (with bogus VRAM flagged), and the Worker Start/Stop button launches/stops the real `rpc-server`. Foundation (`airpcez-core` types/traits/pure logic + platform backends + server + supervisor) is in place for Phase 2 (host role, flag-builder, cockpit) and Phase 3 (suggestion engine).

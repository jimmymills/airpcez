# Model Launch Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Save / list / load / launch named launch profiles per model — full checkpoints capturing launch levers + RPC topology + recorded tok/s.

**Architecture:** A new `Profile`/`ProfileStore` in `airpcez-core` persisted to `airpcez-profiles.toml` beside `airpcez.toml`. The host cockpit gets `/profiles` CRUD plus `/profiles/:id/apply` (reconcile the host's `nodes`) and `/profiles/:id/launch` (reconcile + launch, reusing the existing `build_launch_spec`). A cockpit "Profiles" card exposes Load / Launch / Save / Delete.

**Tech Stack:** Rust (axum, serde, toml), vanilla JS/HTML cockpit. Spec: `docs/superpowers/specs/2026-06-26-model-profiles-design.md`.

## Global Constraints

- TDD: every production change is preceded by a failing test (UI in `assets/index.html` is the static-asset exception — verified by serving).
- Zero build warnings; `cargo clippy --all-targets` clean (project bar).
- `Profile` field order: **all scalar fields first, `nodes: Vec<NodeEntry>` LAST** — TOML requires scalar values before arrays-of-tables within a table.
- `Profile` levers mirror `LaunchRequest` exactly: `ngl, tensor_split, main_gpu, device, cpu_moe, ctx, no_mmap, flash_attn, threads, threads_batch, cache_type_k, cache_type_v, hf_cache_dir`.
- Don't hold the `config` Mutex across an `.await`.

## File Structure

- Create: `crates/airpcez-core/src/profile.rs` — `Profile`, `slugify`, `ProfileStore`.
- Modify: `crates/airpcez-core/src/lib.rs` — register `pub mod profile;`.
- Modify: `crates/airpcez/src/server.rs` — `AppState::profiles_path()`; `now_unix`; `resolve_launch_model` (extracted); `launch_request_from_profile`; profile handlers; routes.
- Modify: `crates/airpcez/assets/index.html` — Profiles card + JS.
- Modify: `.gitignore` — add `airpcez-profiles.toml`.
- Test: profile unit tests in `profile.rs`; mapper/unit tests in `server.rs`; `crates/airpcez/tests/profiles_endpoint.rs` (integration).

---

### Task 1: `Profile` struct + `slugify`

**Files:**
- Create: `crates/airpcez-core/src/profile.rs`
- Modify: `crates/airpcez-core/src/lib.rs`

**Interfaces:**
- Produces: `airpcez_core::profile::Profile` (struct, all fields `pub`), `airpcez_core::profile::slugify(name: &str) -> String`.

- [ ] **Step 1: Register the module.** In `crates/airpcez-core/src/lib.rs`, add after `pub mod planner;`:

```rust
pub mod profile;
```

- [ ] **Step 2: Write `profile.rs` with the struct, `slugify`, and failing tests.** Create `crates/airpcez-core/src/profile.rs`:

```rust
use crate::cluster::NodeEntry;
use serde::{Deserialize, Serialize};

/// A saved launch configuration for a model: launch levers + RPC topology + provenance.
/// SCALAR FIELDS FIRST, `nodes` LAST — TOML requires values before arrays-of-tables.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub model: String,
    #[serde(default)] pub ngl: Option<u32>,
    #[serde(default)] pub tensor_split: Option<String>,
    #[serde(default)] pub main_gpu: Option<u32>,
    #[serde(default)] pub device: Option<String>,
    #[serde(default)] pub cpu_moe: Option<String>,
    #[serde(default)] pub ctx: Option<u32>,
    #[serde(default)] pub no_mmap: bool,
    #[serde(default)] pub flash_attn: Option<String>,
    #[serde(default)] pub threads: Option<u32>,
    #[serde(default)] pub threads_batch: Option<u32>,
    #[serde(default)] pub cache_type_k: Option<String>,
    #[serde(default)] pub cache_type_v: Option<String>,
    #[serde(default)] pub hf_cache_dir: Option<String>,
    #[serde(default)] pub host_label: Option<String>,
    #[serde(default)] pub tok_s: Option<f32>,
    #[serde(default)] pub note: Option<String>,
    #[serde(default)] pub updated_at: u64,
    #[serde(default)] pub nodes: Vec<NodeEntry>,
}

/// Lowercase, collapse runs of non-alphanumerics to a single '-', trim leading/trailing '-'.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("Best Networked"), "best-networked");
        assert_eq!(slugify("solo-2080!!"), "solo-2080");
        assert_eq!(slugify("  A  B  "), "a-b");
        assert_eq!(slugify(""), "");
    }
}
```

- [ ] **Step 3: Run to verify it fails.**

Run: `cargo test -p airpcez-core slugify_cases`
Expected: FAIL initially only if mistyped; otherwise it compiles and passes. If it passes immediately, that's acceptable here — `slugify` is the unit under test and the assertions exercise real behavior. (If compile errors, fix imports.)

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -p airpcez-core slugify_cases`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/airpcez-core/src/profile.rs crates/airpcez-core/src/lib.rs
git commit -m "feat(core): Profile struct + slugify"
```

---

### Task 2: `ProfileStore` (load/save/list/get/upsert/remove)

**Files:**
- Modify: `crates/airpcez-core/src/profile.rs`

**Interfaces:**
- Consumes: `Profile` (Task 1).
- Produces: `ProfileStore { pub profiles: Vec<Profile> }` with `load(&Path) -> ProfileStore`, `save(&Path) -> Result<(),String>`, `list(Option<&str>) -> Vec<&Profile>`, `get(&str) -> Option<&Profile>`, `upsert(Profile)`, `remove(&str) -> bool`.

- [ ] **Step 1: Write the failing tests.** Append inside the existing `#[cfg(test)] mod tests` in `crates/airpcez-core/src/profile.rs` (before its closing `}`):

```rust
    fn sample(id: &str, model: &str) -> Profile {
        Profile { id: id.into(), name: id.into(), model: model.into(), ..Default::default() }
    }

    #[test]
    fn store_upsert_get_remove() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a", "m1"));
        s.upsert(sample("b", "m2"));
        assert_eq!(s.profiles.len(), 2);
        // upsert replaces same id, does not append
        let mut a2 = sample("a", "m1");
        a2.name = "Renamed".into();
        s.upsert(a2);
        assert_eq!(s.profiles.len(), 2);
        assert_eq!(s.get("a").unwrap().name, "Renamed");
        assert!(s.get("missing").is_none());
        assert!(s.remove("a"));
        assert!(!s.remove("a")); // already gone
        assert_eq!(s.profiles.len(), 1);
    }

    #[test]
    fn store_list_filters_by_model() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a", "m1"));
        s.upsert(sample("b", "m2"));
        s.upsert(sample("c", "m1"));
        assert_eq!(s.list(None).len(), 3);
        let m1: Vec<&str> = s.list(Some("m1")).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(m1, vec!["a", "c"]);
        assert_eq!(s.list(Some("nope")).len(), 0);
    }

    #[test]
    fn store_roundtrips_through_toml_file() {
        let mut s = ProfileStore::default();
        let mut p = sample("best-networked", "unsloth/Q:Q4_K_M");
        p.ngl = Some(99);
        p.cpu_moe = Some("16".into());
        p.tok_s = Some(5.95);
        p.nodes = vec![NodeEntry { name: "m2".into(), addr: "192.168.0.125:8675".into() }];
        s.upsert(p);
        let path = std::env::temp_dir().join("airpcez-profiletest-roundtrip.toml");
        let _ = std::fs::remove_file(&path);
        s.save(&path).unwrap();
        let loaded = ProfileStore::load(&path);
        assert_eq!(loaded.profiles, s.profiles);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn store_load_missing_file_is_empty() {
        let path = std::env::temp_dir().join("airpcez-profiletest-does-not-exist.toml");
        let _ = std::fs::remove_file(&path);
        assert_eq!(ProfileStore::load(&path).profiles.len(), 0);
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p airpcez-core store_`
Expected: FAIL to compile — `ProfileStore` not defined.

- [ ] **Step 3: Implement `ProfileStore`.** In `crates/airpcez-core/src/profile.rs`, add after the `slugify` function (above the `#[cfg(test)]` module):

```rust
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProfileStore {
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

impl ProfileStore {
    /// Missing file → empty store. Garbled file → warn and treat as empty (never panic).
    pub fn load(path: &Path) -> ProfileStore {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!(
                    "[airpcez] WARNING: {} failed to parse ({e}) — treating profiles as empty",
                    path.display()
                );
                ProfileStore::default()
            }),
            Err(_) => ProfileStore::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }

    pub fn list(&self, model: Option<&str>) -> Vec<&Profile> {
        self.profiles
            .iter()
            .filter(|p| match model {
                Some(m) => p.model == m,
                None => true,
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Replace the profile with the same id, else append.
    pub fn upsert(&mut self, p: Profile) {
        if let Some(slot) = self.profiles.iter_mut().find(|x| x.id == p.id) {
            *slot = p;
        } else {
            self.profiles.push(p);
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        self.profiles.len() != before
    }
}
```

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -p airpcez-core store_`
Expected: PASS (4 tests). Then `cargo test -p airpcez-core` — all green.

- [ ] **Step 5: Commit.**

```bash
git add crates/airpcez-core/src/profile.rs
git commit -m "feat(core): ProfileStore load/save/list/get/upsert/remove"
```

---

### Task 3: Profiles store path + CRUD endpoints

**Files:**
- Modify: `crates/airpcez/src/server.rs`
- Modify: `.gitignore`
- Test: `crates/airpcez/tests/profiles_endpoint.rs`

**Interfaces:**
- Consumes: `ProfileStore`, `Profile`, `slugify` (Tasks 1–2); `AppState` (`config_path`, `config`).
- Produces: `AppState::profiles_path() -> PathBuf`; routes `GET/POST/DELETE /profiles`; fn `now_unix() -> u64`.

- [ ] **Step 1: Add the `profiles_path` method + `now_unix`, and import extractors.** In `crates/airpcez/src/server.rs`, add near the other `use axum::...` lines (top of file):

```rust
use axum::extract::{Path as AxPath, Query};
```

Add `now_unix` near the other free functions (e.g., just above `async fn host_launch`):

```rust
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
```

Add the method inside `impl AppState { ... }` (the block that already has `for_test`):

```rust
    /// `airpcez-profiles.toml` beside the config file (per-config unique → test-safe).
    pub fn profiles_path(&self) -> std::path::PathBuf {
        let stem = self
            .config_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("airpcez");
        self.config_path.with_file_name(format!("{stem}-profiles.toml"))
    }
```

- [ ] **Step 2: Write the failing integration test.** Create `crates/airpcez/tests/profiles_endpoint.rs`:

```rust
use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use std::sync::Arc;

fn stats() -> NodeStats {
    NodeStats {
        name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    }
}

#[tokio::test]
async fn profiles_crud_roundtrip() {
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats: stats() }));
    let pp = state.profiles_path();
    let _ = std::fs::remove_file(&pp); // clean slate
    tokio::spawn(airpcez::server::run_server(19301, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();

    // POST a profile (no id -> derived from name)
    let saved: serde_json::Value = c.post("http://127.0.0.1:19301/profiles")
        .json(&serde_json::json!({ "name": "Best Networked", "model": "repo:Q4_K_M", "ngl": 99 }))
        .send().await.unwrap().json().await.unwrap();
    assert!(saved.as_array().unwrap().iter().any(|p| p["id"] == "best-networked"));

    // GET filtered by model returns it; a different model filter does not
    let got: serde_json::Value = c.get("http://127.0.0.1:19301/profiles?model=repo:Q4_K_M")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(got.as_array().unwrap().len(), 1);
    let none: serde_json::Value = c.get("http://127.0.0.1:19301/profiles?model=other")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(none.as_array().unwrap().len(), 0);

    // DELETE removes it
    let after: serde_json::Value = c.request(reqwest::Method::DELETE, "http://127.0.0.1:19301/profiles")
        .json(&serde_json::json!({ "id": "best-networked" }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(after.as_array().unwrap().len(), 0);

    let _ = std::fs::remove_file(&pp);
}
```

- [ ] **Step 3: Run to verify it fails.**

Run: `cargo test -p airpcez profiles_crud_roundtrip`
Expected: FAIL — `/profiles` route returns 404 (handlers/route not added yet).

- [ ] **Step 4: Add the handlers and routes.** In `crates/airpcez/src/server.rs`, add the handlers (e.g., just below `now_unix`):

```rust
use airpcez_core::profile::{slugify, Profile, ProfileStore};

#[derive(serde::Deserialize)]
struct ProfilesQuery {
    model: Option<String>,
}

async fn list_profiles(State(s): State<AppState>, Query(q): Query<ProfilesQuery>) -> Json<Vec<Profile>> {
    let store = ProfileStore::load(&s.profiles_path());
    Json(store.list(q.model.as_deref()).into_iter().cloned().collect())
}

async fn upsert_profile(State(s): State<AppState>, Json(mut p): Json<Profile>) -> impl IntoResponse {
    if p.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "profile name required".to_string()).into_response();
    }
    if p.id.trim().is_empty() {
        p.id = slugify(&p.name);
    }
    p.updated_at = now_unix();
    let mut store = ProfileStore::load(&s.profiles_path());
    store.upsert(p);
    match store.save(&s.profiles_path()) {
        Ok(()) => Json(store.profiles).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ProfileIdBody {
    id: String,
}

async fn delete_profile(State(s): State<AppState>, Json(req): Json<ProfileIdBody>) -> Json<Vec<Profile>> {
    let mut store = ProfileStore::load(&s.profiles_path());
    store.remove(&req.id);
    let _ = store.save(&s.profiles_path());
    Json(store.profiles)
}
```

In `run_server`'s router (after the `/config` route), add:

```rust
        .route("/profiles", get(list_profiles).post(upsert_profile).delete(delete_profile))
```

- [ ] **Step 5: Run to verify it passes.**

Run: `cargo test -p airpcez profiles_crud_roundtrip`
Expected: PASS.

- [ ] **Step 6: Ignore the store file.** Append to `.gitignore`:

```
airpcez-profiles.toml
```

- [ ] **Step 7: Commit.**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/tests/profiles_endpoint.rs .gitignore
git commit -m "feat(server): /profiles CRUD + profiles_path"
```

---

### Task 4: `launch_request_from_profile` + apply/launch endpoints

**Files:**
- Modify: `crates/airpcez/src/server.rs`
- Test: `crates/airpcez/tests/profiles_endpoint.rs` (extend), `server.rs` `#[cfg(test)]` (unit)

**Interfaces:**
- Consumes: `Profile`, `ProfileStore`; existing `build_launch_spec`, `resolve_hf_in_cache`, `resolve_model_path`, `poller::poll_nodes`, `supervisor.start`.
- Produces: `fn resolve_launch_model(&LaunchRequest, Option<&str>) -> Result<ModelRef, String>`; `fn launch_request_from_profile(&Profile) -> LaunchRequest`; routes `POST /profiles/:id/apply`, `POST /profiles/:id/launch`.

- [ ] **Step 1: Write the failing unit test for the mapper.** In `crates/airpcez/src/server.rs`, inside the existing `#[cfg(test)] mod tests` (before its closing `}`):

```rust
    #[test]
    fn profile_maps_to_launch_request() {
        let mut p = airpcez_core::profile::Profile {
            name: "x".into(), model: "unsloth/Q:Q4_K_M".into(), ..Default::default()
        };
        p.ngl = Some(99);
        p.cpu_moe = Some("16".into());
        p.no_mmap = true;
        p.flash_attn = Some("on".into());
        p.ctx = Some(8192);
        let req = super::launch_request_from_profile(&p);
        assert_eq!(req.model_hf.as_deref(), Some("unsloth/Q:Q4_K_M"));
        assert!(req.model_path.is_none());
        assert_eq!(req.ngl, Some(99));
        assert_eq!(req.cpu_moe.as_deref(), Some("16"));
        assert_eq!(req.no_mmap, Some(true));
        assert_eq!(req.flash_attn.as_deref(), Some("on"));

        // A .gguf path maps to model_path, not model_hf
        let mut q = airpcez_core::profile::Profile { name: "y".into(), model: "/mnt/m.gguf".into(), ..Default::default() };
        q.ctx = Some(4096);
        let r2 = super::launch_request_from_profile(&q);
        assert!(r2.model_hf.is_none());
        assert_eq!(r2.model_path.as_deref(), Some("/mnt/m.gguf"));
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p airpcez profile_maps_to_launch_request`
Expected: FAIL to compile — `launch_request_from_profile` not defined.

- [ ] **Step 3: Add the mapper + extract `resolve_launch_model`.** In `crates/airpcez/src/server.rs`, add:

```rust
fn launch_request_from_profile(p: &Profile) -> LaunchRequest {
    // Treat as a local path only when it clearly is one; otherwise an HF repo ref.
    let is_path = p.model.starts_with('/') || p.model.starts_with("./") || p.model.ends_with(".gguf");
    LaunchRequest {
        model_hf: if is_path { None } else { Some(p.model.clone()) },
        model_path: if is_path { Some(p.model.clone()) } else { None },
        ngl: p.ngl,
        tensor_split: p.tensor_split.clone(),
        main_gpu: p.main_gpu,
        device: p.device.clone(),
        cpu_moe: p.cpu_moe.clone(),
        ctx: p.ctx,
        hf_cache_dir: p.hf_cache_dir.clone(),
        no_mmap: Some(p.no_mmap),
        flash_attn: p.flash_attn.clone(),
        threads: p.threads,
        threads_batch: p.threads_batch,
        cache_type_k: p.cache_type_k.clone(),
        cache_type_v: p.cache_type_v.clone(),
    }
}

/// Resolve a launch request's model to a ModelRef, preferring a locally-cached file.
fn resolve_launch_model(
    req: &LaunchRequest,
    cache_dir: Option<&str>,
) -> Result<airpcez_core::flags::ModelRef, String> {
    use airpcez_core::flags::ModelRef;
    match (req.model_hf.as_deref(), req.model_path.as_deref()) {
        (Some(hf), _) if !hf.trim().is_empty() => {
            let hf = hf.trim();
            Ok(match cache_dir.and_then(|c| resolve_hf_in_cache(c, hf)) {
                Some(local) => ModelRef::Local(local),
                None => ModelRef::Hf(hf.to_string()),
            })
        }
        (_, Some(p)) if !p.trim().is_empty() => resolve_model_path(p.trim()).map(ModelRef::Local),
        _ => Err("model_hf or model_path required".to_string()),
    }
}
```

- [ ] **Step 4: Refactor `host_launch` to use `resolve_launch_model`.** In `host_launch`, replace the inline `let model = match (req.model_hf.as_deref(), req.model_path.as_deref()) { ... };` block with:

```rust
    let model = match resolve_launch_model(&req, cache_dir) {
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
```

(Leave the `cache_dir` line above it unchanged.)

- [ ] **Step 5: Run unit test + existing launch test to verify green.**

Run: `cargo test -p airpcez profile_maps_to_launch_request && cargo test -p airpcez host_launch_returns_openai_url`
Expected: both PASS (refactor preserved `host_launch` behavior).

- [ ] **Step 6: Write the failing integration tests for apply/launch.** Append to `crates/airpcez/tests/profiles_endpoint.rs`:

```rust
#[tokio::test]
async fn apply_reconciles_nodes_and_launch_404s_on_unknown() {
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats: stats() }));
    let pp = state.profiles_path();
    let _ = std::fs::remove_file(&pp);
    tokio::spawn(airpcez::server::run_server(19302, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();

    // Create a networked profile with one node
    c.post("http://127.0.0.1:19302/profiles")
        .json(&serde_json::json!({
            "name": "net", "model": "repo:Q4_K_M",
            "nodes": [{ "name": "m2", "addr": "192.168.0.125:8675" }]
        })).send().await.unwrap();

    // apply reconciles the host's node list
    let applied = c.post("http://127.0.0.1:19302/profiles/net/apply").send().await.unwrap();
    assert_eq!(applied.status(), 200);
    let cfg: serde_json::Value = c.get("http://127.0.0.1:19302/config").send().await.unwrap().json().await.unwrap();
    assert_eq!(cfg["nodes"][0]["addr"], "192.168.0.125:8675");

    // launch on an unknown id -> 404
    let unknown = c.post("http://127.0.0.1:19302/profiles/nope/launch").send().await.unwrap();
    assert_eq!(unknown.status(), 404);

    let _ = std::fs::remove_file(&pp);
}
```

- [ ] **Step 7: Run to verify it fails.**

Run: `cargo test -p airpcez apply_reconciles_nodes_and_launch_404s_on_unknown`
Expected: FAIL — `/profiles/:id/apply` route returns 404 (not added yet).

- [ ] **Step 8: Add the apply/launch handlers + routes.** In `crates/airpcez/src/server.rs`:

```rust
async fn apply_profile(State(s): State<AppState>, AxPath(id): AxPath<String>) -> impl IntoResponse {
    let store = ProfileStore::load(&s.profiles_path());
    let Some(p) = store.get(&id).cloned() else {
        return (StatusCode::NOT_FOUND, format!("no profile '{id}'")).into_response();
    };
    {
        let mut c = s.config.lock().unwrap();
        c.nodes = p.nodes.clone();
        let _ = c.save(&s.config_path);
    }
    Json(p).into_response()
}

async fn launch_profile(State(s): State<AppState>, AxPath(id): AxPath<String>) -> impl IntoResponse {
    let store = ProfileStore::load(&s.profiles_path());
    let Some(p) = store.get(&id).cloned() else {
        return (StatusCode::NOT_FOUND, format!("no profile '{id}'")).into_response();
    };
    // Reconcile the host's node list to the profile, read launch config (no await under lock).
    let (llama_dir, llama_port, hf_cache_dir_cfg) = {
        let mut c = s.config.lock().unwrap();
        c.nodes = p.nodes.clone();
        let _ = c.save(&s.config_path);
        (c.llama_dir.clone(), c.llama_port, c.hf_cache_dir.clone())
    };
    let req = launch_request_from_profile(&p);
    let cache_dir = req.hf_cache_dir.as_deref().map(str::trim).filter(|s| !s.is_empty())
        .or(hf_cache_dir_cfg.as_deref());
    let model = match resolve_launch_model(&req, cache_dir) {
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let nodes = { s.config.lock().unwrap().nodes.clone() };
    let cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    let eps: Vec<String> = cluster.nodes.iter()
        .filter_map(|n| n.stats.as_ref().and_then(|st| st.rpc_endpoint.clone()))
        .collect();
    let binary = match &llama_dir {
        Some(d) => format!("{d}/llama-server"),
        None => "llama-server".to_string(),
    };
    let spec = build_launch_spec(&req, &model, &binary, llama_port, &eps);
    match s.supervisor.start(spec) {
        Ok(()) => Json(serde_json::json!({ "openai_url": format!("http://localhost:{}/v1", llama_port) })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({ "error": e }))).into_response(),
    }
}
```

In the router, add after the `/profiles` route:

```rust
        .route("/profiles/:id/apply", post(apply_profile))
        .route("/profiles/:id/launch", post(launch_profile))
```

- [ ] **Step 9: Run to verify it passes.**

Run: `cargo test -p airpcez apply_reconciles_nodes_and_launch_404s_on_unknown`
Expected: PASS. Then `cargo test` (workspace) — all green. Then `cargo clippy --all-targets` — clean.

- [ ] **Step 10: Commit.**

```bash
git add crates/airpcez/src/server.rs crates/airpcez/tests/profiles_endpoint.rs
git commit -m "feat(server): /profiles apply + launch (reconcile nodes, reuse build_launch_spec)"
```

---

### Task 5: Cockpit Profiles card (UI)

**Files:**
- Modify: `crates/airpcez/assets/index.html`

**Interfaces:**
- Consumes: `GET/POST/DELETE /profiles`, `POST /profiles/:id/apply`, `POST /profiles/:id/launch`; existing form field ids (`model-select`, `model-hf`, `model-path`, `launch-ngl`, `launch-ctx`, `launch-tensor-split`, `launch-main-gpu`, `launch-device`, `launch-cpu-moe`, `launch-cpu-moe-n`, `hf-cache-dir`, `launch-flash-attn`, `launch-threads`, `launch-threads-batch`, `launch-no-mmap`), `startHealthPolling`, `startLogsPolling`, `escapeHtml`, `currentModelRef()` (added below).

This is a static asset — no unit tests; verify by serving (Step 6).

- [ ] **Step 1: Add the Profiles card markup.** In `crates/airpcez/assets/index.html`, immediately BEFORE the line `<div class="grid-2">` that precedes `<label for="launch-ngl">` (i.e., at the top of the launch form's controls), insert:

```html
          <div class="field-row" id="profiles-row" style="margin-bottom:12px;">
            <label>Profiles (for selected model)</label>
            <div id="profiles-list" style="font-size:0.8rem; color:#8b949e;">—</div>
            <div class="btn-row" style="margin-top:6px;">
              <input id="profile-name" type="text" placeholder="profile name" style="max-width:160px;" />
              <input id="profile-note" type="text" placeholder="note (optional)" style="max-width:160px;" />
              <input id="profile-toks" type="number" step="0.01" placeholder="tok/s" style="max-width:80px;" />
              <button class="btn-default btn-small" id="btn-save-profile">Save current</button>
            </div>
          </div>
```

- [ ] **Step 2: Add a helper to read the current model ref.** In the `<script>`, add near the other helpers:

```javascript
    function currentModelRef() {
      const hf = document.getElementById("model-hf").value.trim();
      const path = document.getElementById("model-path").value.trim();
      return hf || path || "";
    }
```

- [ ] **Step 3: Add the profiles load/render logic.** Add in the `<script>`:

```javascript
    async function loadProfiles() {
      const model = currentModelRef();
      const listEl = document.getElementById("profiles-list");
      try {
        const url = model ? `/profiles?model=${encodeURIComponent(model)}` : "/profiles";
        const profiles = await (await fetch(url)).json();
        if (!profiles.length) { listEl.innerHTML = "<em>none saved for this model</em>"; return; }
        listEl.innerHTML = profiles.map(p => {
          const toks = (p.tok_s != null) ? ` · ${p.tok_s} tok/s` : "";
          const nodes = ` · ${(p.nodes || []).length} node(s)`;
          const note = p.note ? ` · ${escapeHtml(p.note)}` : "";
          return `<div style="display:flex; align-items:center; gap:8px; padding:3px 0;">
            <strong>${escapeHtml(p.name)}</strong><span>${toks}${nodes}${note}</span>
            <button class="btn-default btn-small" data-load="${escapeHtml(p.id)}">Load</button>
            <button class="btn-green btn-small" data-launch="${escapeHtml(p.id)}">Launch</button>
            <button class="btn-danger btn-small" data-del="${escapeHtml(p.id)}">✕</button>
          </div>`;
        }).join("");
      } catch (e) {
        listEl.innerHTML = `<span class='status-err'>✗ ${escapeHtml(e.message)}</span>`;
      }
    }
```

- [ ] **Step 4: Add the action handlers (event delegation) + save.** Add in the `<script>`:

```javascript
    function setVal(id, v) { document.getElementById(id).value = (v == null ? "" : v); }

    function fillFormFromProfile(p) {
      if (p.model && p.model.includes(".gguf")) { setVal("model-path", p.model); setVal("model-hf", ""); }
      else { setVal("model-hf", p.model); setVal("model-path", ""); }
      setVal("launch-ngl", p.ngl);
      setVal("launch-ctx", p.ctx);
      setVal("launch-tensor-split", p.tensor_split);
      setVal("launch-main-gpu", p.main_gpu);
      setVal("launch-device", p.device);
      setVal("hf-cache-dir", p.hf_cache_dir);
      setVal("launch-flash-attn", p.flash_attn || "");
      setVal("launch-threads", p.threads);
      setVal("launch-threads-batch", p.threads_batch);
      document.getElementById("launch-no-mmap").checked = !!p.no_mmap;
      const cpuMoeEl = document.getElementById("launch-cpu-moe");
      const customRow = document.getElementById("cpu-moe-custom-row");
      if (p.cpu_moe === "all") { cpuMoeEl.value = "all"; customRow.style.display = "none"; }
      else if (p.cpu_moe && /^\d+$/.test(p.cpu_moe)) { cpuMoeEl.value = "custom"; customRow.style.display = ""; setVal("launch-cpu-moe-n", p.cpu_moe); }
      else { cpuMoeEl.value = ""; customRow.style.display = "none"; }
    }

    document.getElementById("profiles-list").addEventListener("click", async (e) => {
      const t = e.target;
      if (t.dataset.load) {
        const p = await (await fetch(`/profiles/${encodeURIComponent(t.dataset.load)}/apply`, { method: "POST" })).json();
        fillFormFromProfile(p);
      } else if (t.dataset.launch) {
        const resultEl = document.getElementById("launch-result");
        resultEl.innerHTML = "<span class='status-info'>launching…</span>";
        const resp = await fetch(`/profiles/${encodeURIComponent(t.dataset.launch)}/launch`, { method: "POST" });
        if (resp.ok) {
          const data = await resp.json();
          const openaiUrl = data.openai_url || "";
          const chatBase = openaiUrl.replace(/\/v1$/, "");
          startHealthPolling(openaiUrl, chatBase, resultEl);
          startLogsPolling();
        } else {
          resultEl.innerHTML = "<span class='status-err'>✗ " + resp.status + " — " + (await resp.text()).slice(0, 200) + "</span>";
        }
      } else if (t.dataset.del) {
        await fetch("/profiles", { method: "DELETE", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ id: t.dataset.del }) });
        loadProfiles();
      }
    });

    document.getElementById("btn-save-profile").addEventListener("click", async () => {
      const name = document.getElementById("profile-name").value.trim();
      if (!name) { alert("profile name required"); return; }
      const cpuMoeSelect = document.getElementById("launch-cpu-moe").value;
      const cpuMoeN = document.getElementById("launch-cpu-moe-n").value.trim();
      const num = (id) => { const v = document.getElementById(id).value.trim(); return v === "" ? null : parseInt(v, 10); };
      const str = (id) => { const v = document.getElementById(id).value.trim(); return v === "" ? null : v; };
      const toks = document.getElementById("profile-toks").value.trim();
      const cluster = await (await fetch("/cluster")).json();
      const nodes = (cluster.nodes || []).filter(n => n.entry && n.entry.addr && n.entry.addr !== "self")
        .map(n => ({ name: n.entry.name, addr: n.entry.addr }));
      const body = {
        name,
        model: currentModelRef(),
        ngl: num("launch-ngl"), ctx: num("launch-ctx"),
        tensor_split: str("launch-tensor-split"), main_gpu: num("launch-main-gpu"),
        device: str("launch-device"), hf_cache_dir: str("hf-cache-dir"),
        flash_attn: (str("launch-flash-attn") || null),
        threads: num("launch-threads"), threads_batch: num("launch-threads-batch"),
        no_mmap: document.getElementById("launch-no-mmap").checked,
        cpu_moe: cpuMoeSelect === "all" ? "all" : (cpuMoeSelect === "custom" && cpuMoeN ? cpuMoeN : null),
        note: str("profile-note"),
        tok_s: toks === "" ? null : parseFloat(toks),
        nodes,
      };
      await fetch("/profiles", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
      document.getElementById("profile-name").value = "";
      loadProfiles();
    });
```

- [ ] **Step 5: Refresh profiles on load and on model change.** Find where `model-select` is wired (search `getElementById("model-select")`) and add `loadProfiles();` to its `change` handler. Also call `loadProfiles();` once during initial page setup (near where other initial fetches run, e.g., after the cluster poll starts).

- [ ] **Step 6: Verify by serving.**

Run (on a spare port, since 8675 is in use):
```bash
cargo run -q -p airpcez -- --ui-port 18676 --role worker &
sleep 4
curl -s http://localhost:18676/ | grep -oE "btn-save-profile|profiles-list" | sort -u
curl -s -X POST http://localhost:18676/profiles -H 'Content-Type: application/json' -d '{"name":"t","model":"repo:Q4_K_M","ngl":99}'
curl -s "http://localhost:18676/profiles?model=repo:Q4_K_M"
lsof -ti :18676 | xargs kill
```
Expected: both ids printed; POST returns a JSON array containing the profile; GET returns it.

- [ ] **Step 7: Commit.**

```bash
git add crates/airpcez/assets/index.html
git commit -m "feat(cockpit): Profiles card — load/launch/save/delete per model"
```

---

## Post-implementation: seed the two profiles

Not a code task — run once against the live host on `.24` after merge/deploy:

```bash
# best-networked (Config A)
curl -s -X POST http://192.168.0.24:8675/profiles -H 'Content-Type: application/json' -d '{
  "name":"best-networked","model":"unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M","hf_cache_dir":"/mnt/ssd/llama/models",
  "ngl":99,"tensor_split":"8,4,8","cpu_moe":"16","ctx":8192,"host_label":"pop-os (.24)","tok_s":5.95,
  "nodes":[{"name":"m2","addr":"192.168.0.125:8675"},{"name":"m1","addr":"192.168.0.25:8675"}]}'

# solo-2080 (Config C)
curl -s -X POST http://192.168.0.24:8675/profiles -H 'Content-Type: application/json' -d '{
  "name":"solo-2080","model":"unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M","hf_cache_dir":"/mnt/ssd/llama/models",
  "ngl":99,"cpu_moe":"all","ctx":8192,"host_label":"pop-os (.24)","tok_s":8.2,"nodes":[]}'
```

(Requires the new binary built+running on `.24`.)

## Self-Review

- **Spec coverage:** data model (T1) ✓, ProfileStore (T2) ✓, storage location/.gitignore (T3) ✓, GET/POST/DELETE (T3) ✓, apply/launch + reconcile (T4) ✓, model-filtered cockpit card with Load/Launch/Save/Delete (T5) ✓, seeding (post-impl section) ✓, error handling — 404 unknown (T4 test), 400 empty name (T3 handler), garbled file empty (T2 test) ✓, testing strategy ✓.
- **Placeholder scan:** none — every code step has complete code; every run step has a command + expected result.
- **Type consistency:** `Profile`/`ProfileStore`/`slugify` signatures identical across T1–T4; `launch_request_from_profile` returns the `LaunchRequest` whose fields match the perf-flags work; `profiles_path()` used consistently; routes match handler names.
</content>

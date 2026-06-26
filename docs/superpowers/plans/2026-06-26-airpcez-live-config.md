# airpcez Live Config — CLI flags + in-app Settings menu — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Populate every config value from CLI flags at startup, and let the user view/edit all of them from a cockpit Settings menu — applied live where possible (only the web-UI port needs a restart), persisted to `airpcez.toml`.

**Architecture:** Centralize config into a single `Arc<Mutex<Config>>` shared by `AppState` and the stats provider, so handlers and `/stats` read current values each request. `GET/POST /config` read/replace+save it. CLI flags override the loaded toml at startup. A Settings panel binds to `/config`.

**Tech Stack:** Existing (Rust 2021/stable, axum, serde, toml). No new deps.

## Global Constraints
- Rust edition 2021; stable. `cargo build` + `cargo test` PRISTINE (0 warnings). `airpcez-core` stays OS-free (this work is all in the `airpcez` binary crate).
- Behavior-preserving refactor in Task 1: all existing tests keep passing.
- Never hold the config `Mutex` across an `.await` — lock, clone out the needed values, unlock, then await.
- `Config` (crates/airpcez/src/config.rs) is already `#[serde(default)]`, `Clone`, `Serialize`/`Deserialize`, with fields: `ui_port:u16, rpc_port:u16, llama_port:u16, role:Role, llama_dir:Option<String>, hf_cache_dir:Option<String>, rpc_binary:Option<String>, node_name:String, nodes:Vec<NodeEntry>`, plus `load(&Path)`, `save(&Path)->Result<(),String>`, `rpc_binary_path()->String`.

---

### Task 1: Centralize config into a shared live store (behavior-preserving refactor)

**Files:** `crates/airpcez/src/server.rs` (AppState + handlers + for_test), `crates/airpcez/src/stats_provider.rs` (LocalStats), `crates/airpcez/src/main.rs`, `crates/airpcez/tests/host_health.rs` (and any other test referencing a removed field).

**Interfaces — Produces:**
- `AppState { provider: Arc<dyn StatsProvider>, supervisor: Arc<dyn ProcessBackend>, config: Arc<Mutex<Config>>, http: reqwest::Client, config_path: std::path::PathBuf, bound_ui_port: u16 }`. REMOVE the `nodes`, `llama_dir`, `llama_port`, `hf_cache_dir` fields — those now live in `config`.
- `AppState::for_test(provider)`: `config = Arc::new(Mutex::new(Config::default()))`, `config_path = std::env::temp_dir().join("airpcez-test-config.toml")`, `bound_ui_port = 8675`.
- `LocalStats { config: Arc<Mutex<Config>> }` (replaces its `name/role/llama_dir/rpc_port` fields). In `sample()`: lock config, clone out `node_name, role, llama_dir, rpc_port`, DROP the lock, then run the existing stats gathering (devices, vm_stat, `detect_binary_version(llama_dir)`, `rpc_endpoint = "0.0.0.0:<rpc_port>"`, `name = node_name`, `role`).

**Steps:**
- [ ] **1. Update handlers** to read from `s.config` instead of the removed fields. In each, lock + clone out the values you need BEFORE any `.await`:
  - `cluster_handler`: `let nodes = { s.config.lock().unwrap().nodes.clone() };` then `poll_nodes(&s.http, &nodes).await`.
  - `add_node`: lock config, dedup+push into `config.nodes`, return `config.nodes.clone()`.
  - `remove_node`: lock config, `config.nodes.retain(...)`, return clone. (Keep the `normalize_node_addr` behavior.)
  - `host_launch`: `let (llama_dir, llama_port, hf_cache_dir) = { let c = s.config.lock().unwrap(); (c.llama_dir.clone(), c.llama_port, c.hf_cache_dir.clone()) };` use these; gather nodes the same scoped way.
  - `host_health`: read `llama_port` from `s.config.lock().unwrap().llama_port`.
  - `suggest_handler`: gather nodes from config the scoped way (same as cluster_handler).
- [ ] **2. main.rs**: build the shared config and pass clones:
  ```rust
  let loaded = Config::load(config_path); // CLI overrides arrive in Task 3
  let mut loaded = loaded;
  if worker_mode { loaded.role = Role::Worker; }
  let bound_ui_port = loaded.ui_port;
  let rpc_port = loaded.rpc_port;
  let rpc_bin = loaded.rpc_binary_path();
  let config = Arc::new(Mutex::new(loaded));
  let provider = Arc::new(LocalStats { config: config.clone() });
  let supervisor: Arc<dyn ProcessBackend> = Arc::new(TokioSupervisor::new());
  let state = AppState { provider, supervisor: supervisor.clone(), config: config.clone(),
                         http: reqwest::Client::new(), config_path: config_path.to_path_buf(), bound_ui_port };
  if worker_mode { /* autostart using rpc_bin + rpc_port as today */ }
  airpcez::server::run_server(bound_ui_port, state).await;
  ```
- [ ] **3. Fix tests that referenced removed fields.** Grep `crates/airpcez/tests` for `.llama_port`, `.llama_dir`, `.hf_cache_dir`, `.nodes` on an `AppState`. In `host_health.rs`, `state.llama_port = 1;` becomes `state.config.lock().unwrap().llama_port = 1;`. Update any others similarly. The `nodes_endpoint`/`suggest`/`host_launch` tests use the HTTP API and `for_test`, so they should compile unchanged.
- [ ] **4. Run** `cargo build -p airpcez` (0 warnings) and `cargo test --workspace` (ALL pass — behavior unchanged). Commit: `refactor: centralize config into a shared Arc<Mutex<Config>> (live state)`.

---

### Task 2: `GET /config` + `POST /config` (live-apply + save)

**Files:** `crates/airpcez/src/server.rs`, `crates/airpcez/tests/config_endpoint.rs` (new).

**Interfaces — Consumes:** `AppState { config, config_path, bound_ui_port }`. **Produces:** routes `GET /config`, `POST /config`.

**Steps:**
- [ ] **1. Failing test** (`tests/config_endpoint.rs`): start a server via `AppState::for_test`, `GET /config` → 200 returns a Config JSON with `ui_port == 8675`; `POST /config` with a body whose `llama_port = 9999` → 200, then `GET /config` shows `llama_port == 9999`.
- [ ] **2. Implement:**
  ```rust
  async fn get_config(State(s): State<AppState>) -> Json<crate::config::Config> {
      Json(s.config.lock().unwrap().clone())
  }
  async fn post_config(State(s): State<AppState>, Json(new): Json<crate::config::Config>) -> impl IntoResponse {
      let restart_required = new.ui_port != s.bound_ui_port;
      let save = { let mut c = s.config.lock().unwrap(); *c = new; c.save(&s.config_path) };
      match save {
          Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "saved": true, "restart_required": restart_required }))).into_response(),
          Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "saved": false, "error": e }))).into_response(),
      }
  }
  ```
  Register `.route("/config", get(get_config).post(post_config))`.
- [ ] **3. Run** `cargo test -p airpcez` (new + existing pass), `cargo build` 0 warnings. Commit: `feat: GET/POST /config (live-apply + persist to airpcez.toml)`.

---

### Task 3: CLI flags for every field

**Files:** `crates/airpcez/src/config.rs` (pure parser + test), `crates/airpcez/src/main.rs`.

**Interfaces — Produces:** `pub fn apply_cli_overrides(mut config: Config, args: &[String]) -> Config`.

**Steps:**
- [ ] **1. Failing test** (in `config.rs` tests mod): `apply_cli_overrides(Config::default(), &["--role","host","--ui-port","9000","--llama-dir","/x"].map(String::from))` yields `role == Host`, `ui_port == 9000`, `llama_dir == Some("/x")`.
- [ ] **2. Implement** `apply_cli_overrides`: iterate args as `(flag, value)` pairs; recognize `--ui-port/--rpc-port/--llama-port` (parse u16, ignore on parse error), `--role` ("host"→Host else Worker), `--llama-dir/--rpc-binary/--hf-cache-dir/--node-name` (Some(value)/value). Ignore unknown flags (e.g. `--worker`). Return the mutated config.
- [ ] **3. main.rs**: `let config = apply_cli_overrides(Config::load(config_path), &std::env::args().collect::<Vec<_>>());` then keep the `--worker` role-force + autostart.
- [ ] **4. Run** tests + build (0 warnings). Commit: `feat: --<field> CLI flags populate initial config`.

---

### Task 4: Cockpit Settings menu

**Files:** `crates/airpcez/assets/index.html`.

**Steps (manual-verified UI):**
- [ ] **1. Add a "Settings" section/panel.** On load, `fetch('/config')` and populate inputs: `ui_port`, `rpc_port`, `llama_port` (number), `role` (select worker/host), `llama_dir`, `rpc_binary`, `hf_cache_dir`, `node_name` (text). (Leave `nodes` to the existing cluster Add/remove UI.) A **Save** button reads the inputs, builds a full Config JSON (merge over the last-fetched config so `nodes` is preserved), `POST /config`, and shows "saved" — plus an amber "ui_port change needs a restart" note when the response has `restart_required: true`. Escape interpolated strings with the existing `escapeHtml`. Keep all existing UI intact.
- [ ] **2. Smoke:** `cargo run`, open the cockpit, change `llama_port`, Save → `curl /config` reflects it and `airpcez.toml` is written; change `ui_port` → Save shows the restart note. `cargo test`/`cargo build` still green. Commit: `feat: cockpit Settings menu (view/edit all config, live)`.

## Definition of Done
`airpcez --role host --llama-dir /x --rpc-port 50052 …` populates config at startup; the cockpit Settings menu shows every value and saves changes to `airpcez.toml`, applied live for all fields except `ui_port` (which prompts a restart). Build/test green and pristine.

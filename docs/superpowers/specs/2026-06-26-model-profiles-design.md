# Model launch profiles — design

- **Date:** 2026-06-26
- **Status:** Approved (brainstorming) — pending spec review
- **Branch:** `feat-model-profiles`

## Goal

Let the user **save, list, load, and launch named launch profiles** so a known-good
configuration for a model can be checkpointed and reproduced. Tuning a model means
juggling many levers (`-ngl`, `--tensor-split`, `--cpu-moe`/`--n-cpu-moe`, `-c`,
`--no-mmap`, `-fa`, `--threads`, devices) **and** a cluster topology (which workers,
which host). Today none of that is persisted — every launch is re-typed into the
cockpit form. Profiles make a tuned config a first-class, reproducible artifact.

Seed it with the two configs measured in the 2026-06-26 sweep so there is immediate value.

## Requirements

A profile is a **full checkpoint** capturing three things:

1. **Launch levers** — everything in the current launch form: `model`, `ngl`,
   `tensor_split`, `main_gpu`, `device`, `cpu_moe`, `ctx`, `no_mmap`, `flash_attn`,
   `threads`, `threads_batch`, `cache_type_k`, `cache_type_v`, `hf_cache_dir`.
2. **Topology** — the RPC worker set (`nodes`) that makes a config "networked", plus an
   informational `host_label` recording where it was captured.
3. **Provenance** — an optional recorded `tok_s` and a free-text `note`, plus `updated_at`.

Behavior:

- Profiles are **keyed for a model** — each carries its `model` string and the cockpit
  filters the list to the currently-selected model.
- Two actions per profile: **Load** (pre-fill the form + reconcile the node list, you
  review then Launch) and **Launch** (one-click: reconcile nodes + launch).
- **Save current as profile** captures the current form values + current node list +
  a name/note/optional tok_s the user supplies.

## Non-goals (YAGNI)

- No profile version history / diffing. Upsert overwrites by id.
- No automatic benchmark capture — `tok_s` is user-entered.
- No cross-host profile sync. Profiles live on the host where they are created.
- No CLI surface in this iteration (the HTTP API makes one trivial to add later).
- No per-model directory storage (approach C); a single store file is enough.

## Architecture

### Data model (`airpcez-core`)

```rust
// crates/airpcez-core/src/profile.rs
pub struct Profile {
    pub id: String,                 // stable slug; derived from name if absent (slugify)
    pub name: String,               // display name
    pub model: String,              // hf "repo:quant" or local path — the "for a model" key
    // launch levers (mirror LaunchRequest)
    pub ngl: Option<u32>,
    pub tensor_split: Option<String>,
    pub main_gpu: Option<u32>,
    pub device: Option<String>,
    pub cpu_moe: Option<String>,    // "off" | "all" | "<n>"
    pub ctx: Option<u32>,
    pub no_mmap: bool,
    pub flash_attn: Option<String>, // "on" | "off" | "auto"
    pub threads: Option<u32>,
    pub threads_batch: Option<u32>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub hf_cache_dir: Option<String>,
    // topology
    pub nodes: Vec<NodeEntry>,      // RPC worker set to register (empty = solo)
    pub host_label: Option<String>, // informational: where it was captured
    // provenance
    pub tok_s: Option<f32>,
    pub note: Option<String>,
    pub updated_at: u64,            // unix seconds (server SystemTime), 0 if unknown
}

pub fn slugify(name: &str) -> String; // lowercase, non-alnum -> '-', collapse/trim '-'
```

`NodeEntry` is the existing `airpcez_core::cluster::NodeEntry`. All `Option`/`Vec`
fields use `#[serde(default)]` so partial/older store files still parse.

### Store (`airpcez-core`)

```rust
pub struct ProfileStore { pub profiles: Vec<Profile> }
impl ProfileStore {
    pub fn load(path: &Path) -> ProfileStore;          // missing/garbled file -> empty (warn on garbled)
    pub fn save(&self, path: &Path) -> Result<(),String>;
    pub fn list(&self, model: Option<&str>) -> Vec<&Profile>; // filter by model when Some
    pub fn get(&self, id: &str) -> Option<&Profile>;
    pub fn upsert(&mut self, p: Profile);              // replace same id, else append; stamps updated_at by caller
    pub fn remove(&mut self, id: &str) -> bool;
}
```

TOML file, mirroring `Config` persistence (`toml::to_string_pretty`). A garbled file
warns and is treated as empty (never silently loses the user's other data by panicking).

### Storage location

`airpcez-profiles.toml`, resolved as `config_path.with_file_name("airpcez-profiles.toml")`
so it sits beside `airpcez.toml` (and uses the temp path under `AppState::for_test`).
Added to `.gitignore` alongside `airpcez.toml`. `AppState` gains
`profiles_path: PathBuf`. Handlers load → mutate → save per request (profiles are small
and low-frequency; this avoids adding a second shared lock and any staleness).

### HTTP API (host cockpit, `server.rs`)

| Method & path | Body | Action | Response |
|---|---|---|---|
| `GET /profiles` | — (optional `?model=`) | list, filtered to model when given | `200 [Profile]` |
| `POST /profiles` | `Profile` (`id` optional → `slugify(name)`) | upsert; stamp `updated_at` | `200 [Profile]` (full list) |
| `DELETE /profiles` | `{ "id": "..." }` | remove by id | `200 [Profile]` (full list) |
| `POST /profiles/:id/apply` | — | reconcile host `config.nodes = profile.nodes` (+persist), no launch | `200 Profile` |
| `POST /profiles/:id/launch` | — | apply (reconcile nodes) then build spec from the profile's levers and `supervisor.start` | `200 {openai_url}` / `404` unknown id / `500` start error |

- **Reconcile** = replace `config.nodes` with the profile's `nodes` and persist (same
  persistence as `/nodes`). The user accepted that loading a networked profile
  reconfigures registered workers; a solo profile (`nodes: []`) clears them.
- `:id/launch` reuses the existing pure `build_launch_spec`. A pure helper
  `launch_request_from_profile(&Profile) -> LaunchRequest` maps the profile's levers; it
  is unit-tested, then `build_launch_spec` (already tested) produces the argv.
- `DELETE` takes an `{id}` body to match the existing `/nodes` delete convention.

### Cockpit UI (`assets/index.html`)

A **Profiles** card above the launch form:

- A list/dropdown of profiles for the **currently-selected model** (re-fetched when the
  model select changes), each row showing name, `tok_s` (if set), node count, and note.
- Per row: **Load** (`POST /profiles/:id/apply` → pre-fill all form fields from the
  returned profile, including the new perf fields and the `cpu_moe` number) and
  **Launch** (`POST /profiles/:id/launch` → reuse the existing health/log polling).
- A **Save current as profile…** control: prompts for name + optional note + optional
  tok_s, gathers the current launch-form values and the current node list, `POST /profiles`.
- A **Delete** affordance per row (`DELETE /profiles {id}`).

UI is the static-asset exception to TDD; verified by serving and inspecting (as in the
perf-flags work).

## Seeding

After the feature ships and is running on `.24`, create two real profiles via
`POST /profiles` (no hardcoded defaults compiled into the binary):

1. **`best-networked`** — `model` `unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M`,
   `nodes` = [`m2`@192.168.0.125:8675, `m1`@192.168.0.25:8675], `ngl` 99,
   `tensor_split` "8,4,8", `cpu_moe` "16", `ctx` 8192, `hf_cache_dir`
   `/mnt/ssd/llama/models`, `host_label` "pop-os (.24)", `tok_s` 5.95.
2. **`solo-2080`** — same model + `hf_cache_dir`, `nodes` = [], `ngl` 99,
   `cpu_moe` "all", `ctx` 8192, `host_label` "pop-os (.24)", `tok_s` 8.2.

## Error handling

- Unknown profile id on `apply`/`launch` → `404`.
- Garbled `airpcez-profiles.toml` → warn, treat as empty (don't panic, don't clobber).
- `launch` start failure (e.g., missing `llama-server`) → `500` with the error string
  (same as `/host/launch`).
- `POST /profiles` with an empty `name` → `400` (a profile needs a display name to slug).

## Testing strategy (TDD)

**Core (`airpcez-core`):**
- `slugify` cases (spaces, punctuation, case, collapse/trim dashes).
- `ProfileStore`: upsert-new vs upsert-replace-same-id; `get`; `remove` (hit/miss);
  `list` with and without the `model` filter; TOML round-trip; missing file → empty.

**Server (`airpcez`):**
- `launch_request_from_profile` maps every lever correctly (pure unit test).
- `POST /profiles` then `GET /profiles` returns it; `?model=` filters; `DELETE` removes.
- `POST /profiles/:id/apply` sets `config.nodes` to the profile's nodes.
- `POST /profiles/:id/launch` on an unknown id → 404; on a known id with a bogus binary
  → 500 (proves wiring), and reconciles nodes first.

**UI:** served and inspected; not unit-tested.

## Risks / notes

- `:id/launch` mutating + persisting `config.nodes` is intended but surprising; the UI
  must make the node change visible (the cluster panel already re-polls every 2s).
- Profiles store device/topology strings verbatim; they are not validated against the
  live cluster at save time (a profile can reference a now-absent node — launch then
  surfaces the normal RPC connect error, which is acceptable).
</content>

# Cluster Memory-Totals Strip — Design Spec

**Date:** 2026-06-26
**Status:** Approved (brainstorming complete; ready for implementation planning)

## Overview

Add a summary strip to the top of the cockpit's cluster panel showing three aggregate
memory figures across the cluster — **System RAM**, **VRAM**, and **Pool** — each as
`free / total`. The point of interest is **Pool**: the real combined memory available to
the cluster, computed so that **unified memory is never counted twice** (an Apple-Silicon
node's Metal VRAM is carved from its system RAM, so adding RAM + VRAM would double-count it).

## Goals

- One glance shows the cluster's aggregate capacity (ideal) and current free (now).
- **Pool** correctly de-duplicates unified memory: `RAM + discrete-GPU VRAM only`.
- Keep the aggregation logic **pure and unit-tested** in `airpcez-core`, consistent with
  the project's "hard logic is pure" ethos; the cockpit just renders it.
- No change to what nodes report — works with the existing `NodeStats`/`DeviceStats`.

## Non-goals

- No new per-node fields or `/stats` schema change (unified-ness is inferred from `kind`).
- No persistence, history, or charts — a live strip only.

## Key decisions (from brainstorming)

| Decision | Choice |
|---|---|
| Unified detection | **Infer from `DeviceKind`**: `Metal` ⇒ unified (VRAM ⊂ RAM); `Cuda`/`Other` ⇒ discrete |
| What to show | **Both** total and free for each of RAM / VRAM / Pool (ideal vs current) |
| Where the logic lives | **Pure fn in `airpcez-core`**, attached to the `/cluster` response |
| Scope of the sum | **Reachable nodes only**, including the host ("self") |
| Unreliable devices | **Excluded** from VRAM and Pool (the Vulkan bogus/overflow-VRAM guard) |

## The calculation

`cluster_memory_totals(&ClusterStatus) -> MemoryTotals`, summing over nodes that are
reachable and have stats (the host's "self" node included):

- **RAM**: `ram_total_mib = Σ ram_total`, `ram_free_mib = Σ ram_free`.
- **VRAM**: over GPU devices with `reliable == true` and `kind != Cpu`:
  `vram_total_mib = Σ vram_total`, `vram_free_mib = Σ vram_free`.
- **Pool** (de-duped): start from RAM, then add **discrete** GPU memory only —
  devices with `reliable == true` and `kind ∈ {Cuda, Other}`:
  - `pool_total_mib = ram_total_mib + Σ_discrete vram_total`
  - `pool_free_mib  = ram_free_mib  + Σ_discrete vram_free`
  - `Metal` devices are deliberately **not** added (their VRAM is already inside RAM).

Rationale for the `kind`-based rule: for this cluster, all Apple/Metal nodes are
Apple-Silicon (unified) and the only discrete VRAM comes from the NVIDIA/`Cuda` box.
Known edge case (not present in this cluster): an Intel Mac with a discrete GPU would
report `Metal` yet be discrete, and would be under-counted in Pool. Documented, accepted.

## Data structures & wiring

- New in `airpcez-core/src/cluster.rs`:
  ```rust
  #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
  pub struct MemoryTotals {
      pub ram_total_mib: u64,  pub ram_free_mib: u64,
      pub vram_total_mib: u64, pub vram_free_mib: u64,
      pub pool_total_mib: u64, pub pool_free_mib: u64,
  }
  pub fn cluster_memory_totals(status: &ClusterStatus) -> MemoryTotals
  ```
- `ClusterStatus` gains `#[serde(default)] pub totals: MemoryTotals` (default keeps existing
  JSON round-trip tests valid). `cluster_handler` computes it after `warnings`:
  `let t = cluster_memory_totals(&cluster); cluster.totals = t;`.
- `/cluster` JSON now carries `totals`; nothing else about the endpoint changes.

## UI

A compact 3-cell strip at the top of `.cluster-panel` in `crates/airpcez/assets/index.html`,
above the warnings banner and node list, populated from `data.totals` on each `/cluster`
poll (every 2s). Each cell: a label (**System RAM** / **VRAM** / **Pool**) and
`{free} free / {total} total` formatted with the existing `fmtMib` helper. The Pool cell
carries a short "(dedup)" hint so it's clear why Pool ≠ RAM + VRAM. Styling reuses the
existing stat/card classes; no new dependencies.

## Testing

- **Pure-fn unit tests** in `cluster.rs`: a mixed cluster fixture —
  - an Apple-Silicon node (unified `Metal`, e.g. ram 16384 / vram 12288),
  - an NVIDIA node (discrete `Cuda`, e.g. ram 32000 / vram 8192),
  - a node with an **unreliable** device (overflow VRAM),
  - and an **unreachable** node (no stats) —
  asserting: RAM = Σ reachable ram; VRAM excludes the unreliable device; Pool = total RAM +
  the discrete Cuda VRAM only (Metal **not** added, unreliable **not** added); and the
  unreachable node contributes nothing. Plus a zero/empty-cluster case → all zeros.
- **Endpoint test:** extend the `/cluster` integration test to assert the response includes
  `totals` with the expected aggregate for its fixture nodes.

## Scope

Three files: `airpcez-core/src/cluster.rs` (struct + fn + tests), `crates/airpcez/src/server.rs`
(compute in `cluster_handler`; endpoint test), `crates/airpcez/assets/index.html` (the strip).
On branch `dashboard-pool-totals`.

# Planner GPU-first tiering — design

**Date:** 2026-06-26
**Status:** approved (design); pending implementation plan
**Area:** `crates/airpcez-core/src/planner.rs` (`suggest_plan`), worker config, host-launch wiring

## Problem

Adding the first CPU-only node (`intel-mbp`, 32 GB RAM, no usable GPU) exposed gaps in
`suggest_plan` (the "Suggest settings" button → `POST /suggest`):

1. **`host_hint` recommends the CPU-only node.** It picks the node with the most *total
   RAM*; `intel-mbp` (32 GB) now wins over the GPU host `.24` (32 GB CUDA box), so every
   suggestion says "run llama-server on intel-mbp" — a GPU-less box. Hosting there throws
   away direct GPU use.
2. **`cpu_pool` over-counts.** It sums *every* reachable node's free RAM, including the
   CPU-only node, even though CPU-resident layers actually run only on the **host** node's
   CPU. The "fits / headroom" verdict is inflated by RAM the plan can't use.
3. **No way to actually use the CPU node.** Even when a model is too big for GPU + host
   RAM, the planner has no path to spill onto the CPU node.

The GPU-first *layer packing* is already correct: models that fit go fully on GPU; the 70B
fills the GPU pool and spills only the remainder to host CPU. This design keeps that and
adds an explicit capacity hierarchy around it.

## Goal

`suggest_plan` should place a model by a strict priority hierarchy:

1. **GPUs** (all GPU nodes) — fill first
2. **Host memory** (the RAM of the node running llama-server) — overflow lands here
3. **CPU-only node(s)** over RPC — engaged **only** when GPU + host RAM cannot hold the model

## Non-goals

- Changing the GPU layer-packing heuristics or the existing safety margins (dense `10/11`,
  MoE `+10%`) that prevent llama.cpp's "failed to fit params". Those stay.
- A general multi-device bin-packer. One CPU-spillover tier is enough (YAGNI).
- Making distributed CPU spillover *fast*. Per the cluster's Qwen3 finding, RPC-to-CPU over
  gigabit is latency-bound and slow; tier 3 is a "runs instead of failing" last resort.

## Key finding that makes tier 3 feasible

A CPU/Accelerate rpc-server, by default, serves the **BLAS (Accelerate)** device, which
reports **0 MiB** — unusable for offload — because `get_devices()` in `tools/rpc/rpc-server.cpp`
prefers non-CPU devices and only falls back to the CPU device when there are none. The
RAM-backed CPU device exists and is exposable:

```
BLAS: Accelerate (0 MiB, 0 MiB free)            # served by default — unusable
CPU:  Intel i7-9750H (32768 MiB, 32768 free)    # NOT served by default — what we want
```

Running rpc-server with **`-d CPU`** serves the 32 GB CPU device. BLAS stays loaded, so the
CPU backend still uses Accelerate for matmuls.

## Design

### Capacity tiers (in `suggest_plan`)

- **Host node** = the cluster node that will run llama-server: the node with `role == Host`,
  else `nodes[0]` (`suggest_handler` inserts `self` at index 0). For the live cluster this
  resolves to `.24`.
- **Tier 1 — GPU pool:** sum over *all* reachable nodes of each reliable GPU device's
  `vram_free - GPU_HEADROOM_MIB`. (Unchanged from today, including: devices flagged
  `reliable == false` — e.g. the bogus-Vulkan-VRAM case — stay excluded and keep their
  `exclude_notes` entry.)
- **Tier 2 — host CPU:** the host node's `ram_free - CPU_HEADROOM_MIB`, minus the host's own
  unified-memory carve-out if the host is Apple Silicon (existing `unified_vram` logic,
  applied only to the host node now).
- **Tier 3 — remote CPU pool:** sum over reachable **non-host nodes that expose no reliable
  GPU** (CPU-only workers) of `ram_free - CPU_HEADROOM_MIB`. Uses the conservative airpcez
  `ram_free` (reclaimable RAM), not the rpc-server's optimistic "total = free" report.

### Fit verdict

- `primary = gpu_pool + host_cpu`. Compute `required = total_mib + kv_mib(...)`.
  - `required + 10% <= primary` → **Fits** (GPU + host); tier 3 **not** involved.
  - `required <= primary` → **Tight** (GPU + host); tier 3 not involved.
  - else → tier 3 needed:
    - `required <= primary + remote_cpu_pool` → **Fits/Tight via RPC CPU**, wire tier 3.
    - else → **WontFit** even with CPU nodes → recommend a smaller quant.
- The verdict string names the tiers, e.g. `fits — 31 GB GPU + 27 GB host CPU` or, on
  overflow, `tight — +6000 MiB on intel-mbp via RPC (slow last resort)`.

### Plan generation

- **GPU + host CPU (Tiers 1–2):** unchanged. Dense fills GPU (`ngl` from `gpu_pool`),
  remainder on host CPU. MoE keeps attention on GPU (`ngl = n_layers`), experts spill to host
  CPU via partial `--n-cpu-moe`. For the live cluster (~52 GB across Tiers 1+2) every catalog
  model fits here and `intel-mbp` is never touched.
- **Tier 3 (overflow only):** add the CPU-only node(s) to `--rpc`, assign their CPU device a
  `--tensor-split` share sized to the overflow, and bump `ngl` to cover those layers (host CPU
  keeps the `n_layers - ngl` non-offloaded layers; the remote CPU device holds its split
  share). Dense is the primary supported tier-3 path. MoE tier-3 (routing experts to a remote
  CPU device, which `--n-cpu-moe` cannot target) is harder; if verification shows it's not
  clean, MoE overflow degrades to the "report it" verdict rather than auto-wiring.

### `host_hint`

Replace "max total RAM node" with: the host is the host node (above). Emit a confirmation,
and only a **warning** when that node has no reliable GPU ("host node X has no GPU — run
llama-server on a GPU node"). Never recommend a CPU-only node.

### Worker reconfig

CPU-only workers must serve their CPU device. Couple it to the existing GPU-suppression flag:
when `advertise_gpu == false`, `Config::rpc_device_filter()` returns `"CPU"` (so a CPU-only
worker automatically serves its RAM-backed device over RPC). `intel-mbp`'s running worker is
restarted to pick this up. Explicit `rpc_device = "CPU"` remains an override.

### Integration: carrying tier-3 nodes to launch

`suggest_plan`'s output is advisory; the actual `--rpc`/`--device` flags are built at
`POST /host/launch`. Tier 3 requires the launch to include the CPU node(s) as an RPC **CPU**
device slot in the correct tensor-split position. The `Plan` struct gains the information the
launch needs (e.g. the ordered list of participating RPC nodes and which are CPU-backed), and
`host_launch` honors it. Exact shape decided in the implementation plan.

## Verification gate (do this BEFORE wiring tier 3 into the planner)

Empirically confirm llama.cpp offloads cleanly to a remote **CPU** rpc-server device:

1. Restart `intel-mbp` rpc-server with `-d CPU` (serves 32 GB).
2. From the host, run llama-server `--rpc 192.168.0.111:50052` with a small model and a
   device/tensor-split assignment that forces some layers onto the RPC CPU slot.
3. Confirm weights land on the RPC device and a token decodes.

If clean → implement tier-3 auto-wiring (changes above). If flaky → tier 3 degrades to
"report it" (verdict only), and we still ship the certain wins: host selection (#1), tiered
accounting (#2), and the worker `-d CPU` capability (#5/reconfig).

## Affected code

- `crates/airpcez-core/src/planner.rs` — `suggest_plan` (tiers, fit, `host_hint`),
  possibly new `Plan` fields for tier-3 wiring.
- `crates/airpcez/src/config.rs` — `rpc_device_filter()` returns `"CPU"` when
  `advertise_gpu == false`.
- `crates/airpcez/src/server.rs` (`host_launch`) and/or `crates/airpcez-core/src/flags.rs` —
  honor tier-3 RPC CPU nodes when building launch flags.
- Worker deploy: `intel-mbp` `airpcez.toml` / restart.

## Testing

Unit tests on `suggest_plan` (pure function over `ClusterStatus`):

- Host selection picks the GPU host, never a CPU-only node; `host_hint` warns when the host
  has no GPU.
- CPU-only node is excluded from the primary pool; verdict not inflated by its RAM.
- Tier 3 engages only when `required > gpu_pool + host_cpu`, and `WontFit` when it exceeds all
  three tiers.
- Tier-3 plan carries the CPU node(s) and a tensor-split share.

Plus the manual end-to-end verification gate above (documented, not automated).

## Risks / open questions

- **RPC-to-CPU offload correctness** — the verification gate resolves this; tier 3 is gated on
  it.
- **MoE tier-3** — experts can't be targeted by `--n-cpu-moe` to a remote device; may stay
  "report it" for MoE.
- **rpc-server reports CPU free = total** (optimistic) vs airpcez's conservative `ram_free` —
  planner uses the conservative figure, so this is safe.

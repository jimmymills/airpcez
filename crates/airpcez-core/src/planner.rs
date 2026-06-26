use crate::cluster::ClusterStatus;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ModelMeta {
    pub total_mib: u64,
    pub n_layers: u32,
    pub is_moe: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Fits,
    Tight,
    WontFit,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct FitVerdict {
    pub fit: Fit,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Plan {
    pub ngl: u32,
    pub tensor_split: Option<String>,
    pub cpu_moe: Option<String>,
    pub exclude_notes: Vec<String>,
    pub fit: FitVerdict,
    pub gpu_pool_mib: u64,
    pub cpu_pool_mib: u64,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Recommended flash-attention setting for the launch (e.g. "on").
    #[serde(default)]
    pub flash_attn: Option<String>,
    /// Recommend `--no-mmap` (true when experts are offloaded to CPU).
    #[serde(default)]
    pub no_mmap: bool,
    /// Which node to run llama-server on, and why (highest-RAM node).
    #[serde(default)]
    pub host_hint: Option<String>,
    /// Tier-3 capacity: reclaimable RAM (MiB) of CPU-only worker nodes, usable via RPC.
    /// 0 when there are none.
    #[serde(default)]
    pub remote_cpu_pool_mib: u64,
    /// rpc-server endpoints of CPU-only nodes to engage as last-resort CPU spillover.
    /// Empty unless the model overflows GPU + host CPU (tier 3 engaged).
    #[serde(default)]
    pub rpc_cpu_nodes: Vec<String>,
}

/// Rough KV-cache size: ~0.125 MiB per layer per 1024 context tokens.
/// A heuristic for the fit check, not an exact figure. Note: the estimate
/// trends LOW at high context (e.g. 32k+) because it ignores GQA ratios and
/// quantized-KV savings; the Fits 10% headroom margin partially absorbs this,
/// so the formula is deliberately rough rather than over-precise.
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

const GPU_HEADROOM_MIB: u64 = 1024;
const CPU_HEADROOM_MIB: u64 = 2048;

/// Heuristic launch planner. Never trusts a device flagged `reliable == false`
/// (e.g. the Vulkan overflow); reserves per-tier headroom; sizes -ngl to the
/// real reliable GPU pool and verdicts the fit against GPU + CPU memory.
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
    let mut tensor_split = ratio_string(&splits);
    let (mut ngl, cpu_moe, mut no_mmap) = if meta.is_moe {
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
        if meta.is_moe {
            // MoE experts spill to host CPU via --n-cpu-moe, which can't target a remote CPU
            // device — so a MoE that overflows GPU + host is reported, not auto-wired.
            (Fit::Tight, format!("tight — overflow needs CPU-node spillover, but MoE experts can't be auto-routed to a remote CPU (+{} MiB); use a smaller quant or add host RAM{}",
                required - primary, roomiest_suffix))
        } else {
            // Dense: auto-route the overflow onto the CPU node(s) over RPC. Fill host CPU
            // (Tier 2) first; only the remainder becomes remote-CPU (Tier 3) layers.
            rpc_cpu_nodes = cpu_node_eps.clone();
            let gpu_layers = ngl; // dense ngl == layers that fit on GPU
            let host_cpu_layers = (host_cpu / per_layer) as u32;
            let remote_layers = meta.n_layers
                .saturating_sub(gpu_layers.saturating_add(host_cpu_layers))
                .max(1);
            let n_cpu = cpu_node_eps.len().max(1) as u64;
            let remote_mib_per_node = remote_layers as u64 * per_layer / n_cpu;
            for _ in 0..cpu_node_eps.len() {
                splits.push(remote_mib_per_node);
            }
            tensor_split = ratio_string(&splits);
            ngl = gpu_layers.saturating_add(remote_layers).min(meta.n_layers);
            no_mmap = true;
            (Fit::Tight, format!("tight — {} layers (+{} MiB) spill to CPU node(s) via RPC (slow last resort); {} GPU + {} host CPU + {} remote CPU{}",
                remote_layers, remote_layers as u64 * per_layer, gpu_pool, host_cpu, remote_cpu, roomiest_suffix))
        }
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

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn plan_json_roundtrips() {
        let p = Plan {
            ngl: 40, tensor_split: Some("12,11".into()), cpu_moe: None,
            exclude_notes: vec!["drop Vulkan0".into()],
            fit: FitVerdict { fit: Fit::Tight, detail: "tight".into() },
            gpu_pool_mib: 21000, cpu_pool_mib: 26000,
            warnings: vec![],
            flash_attn: Some("on".into()), no_mmap: true, host_hint: Some("host on big-box".into()),
            remote_cpu_pool_mib: 12000, rpc_cpu_nodes: vec!["10.0.0.9:50052".into()],
        };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(p, serde_json::from_str::<Plan>(&j).unwrap());
        assert!(j.contains("\"fit\":\"tight\""));
    }

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

    #[test]
    fn moe_partial_cpu_offload_when_gpu_short() {
        // 21 GB MoE, 48 layers, on a single 8 GB GPU: most experts must spill to CPU —
        // but as a PARTIAL --n-cpu-moe count, never the all-or-nothing "all" that swamped the host.
        let cluster = ClusterStatus {
            nodes: vec![node("box", 40000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)])],
            warnings: vec![],
        };
        let meta = ModelMeta { total_mib: 21000, n_layers: 48, is_moe: true };
        let p = suggest_plan(&cluster, &meta, 8192);
        assert_eq!(p.ngl, 48, "MoE keeps every layer's attention on GPU (-ngl = n_layers)");
        let n: u32 = p.cpu_moe.as_deref().expect("cpu_moe set for MoE")
            .parse().expect("cpu_moe is a partial layer count, not all/off");
        assert!(n > 0 && n < 48, "partial offload expected, got {n}");
        assert!(n >= 20, "GPU holds little here, so most expert layers offload: {n}");
        assert!(p.no_mmap, "--no-mmap recommended when experts live on CPU");
    }

    #[test]
    fn moe_no_offload_when_gpu_roomy() {
        // Whole 21 GB MoE on a 28 GB GPU — comfortably above the weights + a ~10% margin for
        // KV/compute/context → no CPU experts, mmap fine.
        let cluster = ClusterStatus {
            nodes: vec![node("box", 40000, vec![gpu("CUDA0", DeviceKind::Cuda, 30000, 28000, true)])],
            warnings: vec![],
        };
        let meta = ModelMeta { total_mib: 21000, n_layers: 48, is_moe: true };
        let p = suggest_plan(&cluster, &meta, 8192);
        assert_eq!(p.cpu_moe.as_deref(), Some("off"));
        assert!(!p.no_mmap);
        assert_eq!(p.ngl, 48);
    }

    #[test]
    fn moe_offloads_when_gpu_margin_thin() {
        // Regression: GPU pool only marginally exceeds the model WEIGHTS. The old planner chose
        // cpu_moe=off (packing the whole model onto GPUs with ~0 headroom), leaving no room for
        // KV cache, compute buffers, or per-device context — so llama.cpp's param-fit failed
        // ("failed to fit params"). The planner must keep a GPU margin and instead offload a few
        // expert layers to CPU.
        let cluster = ClusterStatus {
            // gpu_pool = 22500 - 1024 = 21476, only ~2% above the 21000 weights.
            nodes: vec![node("box", 40000, vec![gpu("CUDA0", DeviceKind::Cuda, 24576, 22500, true)])],
            warnings: vec![],
        };
        let meta = ModelMeta { total_mib: 21000, n_layers: 48, is_moe: true };
        let p = suggest_plan(&cluster, &meta, 8192);
        assert_eq!(p.ngl, 48);
        let n: u32 = p.cpu_moe.as_deref().expect("cpu_moe set")
            .parse().expect("thin GPU margin must offload a partial --n-cpu-moe, not 'off'");
        assert!(n > 0 && n < 48, "expected a small partial offload, got {n}");
        assert!(p.no_mmap, "--no-mmap recommended when experts live on CPU");
    }

    #[test]
    fn dense_offloads_layers_when_gpu_margin_thin() {
        // Dense model whose weights barely fit on GPU. Like the MoE case, the planner must leave
        // ~10% GPU headroom (KV/compute/context) by keeping a few layers on CPU (lower ngl) rather
        // than packing ngl = n_layers with zero margin → "failed to fit params".
        let cluster = ClusterStatus {
            // gpu_pool = 22500 - 1024 = 21476, only ~2% above the 21000 weights.
            nodes: vec![node("box", 40000, vec![gpu("CUDA0", DeviceKind::Cuda, 24576, 22500, true)])],
            warnings: vec![],
        };
        let meta = ModelMeta { total_mib: 21000, n_layers: 48, is_moe: false };
        let p = suggest_plan(&cluster, &meta, 8192);
        assert!(p.cpu_moe.is_none(), "dense models use -ngl only, not --cpu-moe");
        assert!(p.ngl > 0 && p.ngl < 48, "thin GPU margin must keep layers on CPU, got ngl={}", p.ngl);
    }

    #[test]
    fn recommends_flash_attn_and_roomiest_host() {
        let cluster = ClusterStatus {
            nodes: vec![
                node("small-mac", 16000, vec![gpu("MTL0", DeviceKind::Metal, 12000, 10000, true)]),
                host("big-box",   64000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
            ],
            warnings: vec![],
        };
        let meta = ModelMeta { total_mib: 21000, n_layers: 48, is_moe: true };
        let p = suggest_plan(&cluster, &meta, 8192);
        assert_eq!(p.flash_attn.as_deref(), Some("on"));
        let hint = p.host_hint.expect("host hint present");
        assert!(hint.contains("big-box"), "should recommend the highest-RAM node: {hint}");
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
    fn tier3_dense_appends_remote_share_and_raises_ngl() {
        // Dense 20 GB / 40 layers; GPU ~7 GB + host CPU ~8 GB can't hold it, CPU node (33 GB) can.
        let cluster = ClusterStatus { nodes: vec![
            host("gpu-host", 10000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
            cpu_worker("cpu-box", 33000, "192.168.0.111:50052"),
        ], warnings: vec![] };
        let meta = ModelMeta { total_mib: 20000, n_layers: 40, is_moe: false };
        let p = suggest_plan(&cluster, &meta, 4096);
        // tensor_split gains a trailing CPU-node entry: host GPU + remote CPU = 2 fields.
        assert_eq!(p.tensor_split.as_deref().unwrap().split(',').count(), 2);
        assert_eq!(p.rpc_cpu_nodes, vec!["192.168.0.111:50052".to_string()]);
        assert!(p.no_mmap, "offloaded weights -> --no-mmap");
        // ngl now covers GPU layers + remote-CPU layers, i.e. more than the GPU-only count.
        let gpu_only_ngl = ((8000u64 - 1024) * 10 / 11 / (20000 / 40)) as u32;
        assert!(p.ngl > gpu_only_ngl && p.ngl <= 40, "ngl={} gpu_only={}", p.ngl, gpu_only_ngl);
    }

    #[test]
    fn tier3_moe_does_not_autoroute() {
        // Same overflow, but MoE: experts can't be routed to a remote CPU -> report, don't wire.
        let cluster = ClusterStatus { nodes: vec![
            host("gpu-host", 10000, vec![gpu("CUDA0", DeviceKind::Cuda, 8192, 8000, true)]),
            cpu_worker("cpu-box", 33000, "192.168.0.111:50052"),
        ], warnings: vec![] };
        let meta = ModelMeta { total_mib: 20000, n_layers: 40, is_moe: true };
        let p = suggest_plan(&cluster, &meta, 4096);
        assert!(p.rpc_cpu_nodes.is_empty(), "MoE overflow is reported, not auto-wired");
        assert_eq!(p.tensor_split.as_deref().unwrap().split(',').count(), 1, "no appended CPU share for MoE");
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
}

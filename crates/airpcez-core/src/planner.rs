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
    let mut gpu_pool = 0u64;
    let mut splits: Vec<u64> = Vec::new();
    let mut exclude_notes = Vec::new();
    let mut cpu_pool = 0u64;
    let mut roomiest: Option<(String, u64)> = None;
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
                if roomiest.as_ref().map_or(true, |(_, best)| usable > *best) {
                    roomiest = Some((format!("{}/{}", n.entry.name, d.name), usable));
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

    let roomiest_suffix = match &roomiest {
        Some((name, mib)) => format!(" — roomiest GPU {name} {mib} MiB"),
        None => " — no reliable GPU detected".to_string(),
    };

    let required = meta.total_mib + kv_mib(meta.n_layers, ctx);
    let pool = gpu_pool + cpu_pool;
    let (fit, detail) = if required + required / 10 <= pool {
        (Fit::Fits, format!("fits — ~{} MiB headroom across {} MiB GPU + {} MiB CPU{}",
            pool.saturating_sub(required), gpu_pool, cpu_pool, roomiest_suffix))
    } else if required <= pool {
        (Fit::Tight, format!("tight — needs {} MiB, pool is {} MiB ({} GPU + {} CPU){}",
            required, pool, gpu_pool, cpu_pool, roomiest_suffix))
    } else {
        (Fit::WontFit, format!("won't fit — needs {} MiB but pool is only {} MiB ({} GPU + {} CPU); add memory or use a smaller quant{}",
            required, pool, gpu_pool, cpu_pool, roomiest_suffix))
    };
    Plan { ngl, tensor_split, cpu_moe, exclude_notes,
        fit: FitVerdict { fit, detail }, gpu_pool_mib: gpu_pool, cpu_pool_mib: cpu_pool,
        warnings: Vec::new() }
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
}

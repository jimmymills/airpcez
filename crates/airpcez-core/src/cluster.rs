use crate::model::{DeviceKind, NodeStats};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct NodeEntry {
    pub name: String,
    pub addr: String,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DeviceStats, DeviceKind, NodeStats, Role};

    fn snap(name: &str, ver: Option<&str>, reachable: bool) -> NodeSnapshot {
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
}

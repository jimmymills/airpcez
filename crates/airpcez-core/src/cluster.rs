use crate::model::NodeStats;
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
    use crate::model::Role;

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
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;
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
}

use airpcez_core::cluster::{ClusterStatus, NodeEntry, NodeSnapshot};
use airpcez_core::model::NodeStats;
use std::time::Duration;

pub async fn poll_nodes(client: &reqwest::Client, nodes: &[NodeEntry]) -> ClusterStatus {
    let mut snapshots = Vec::with_capacity(nodes.len());
    for node in nodes {
        let url = format!("http://{}/stats", node.addr);
        let snap = match client.get(&url).timeout(Duration::from_secs(2)).send().await {
            Ok(resp) => match resp.json::<NodeStats>().await {
                Ok(stats) => NodeSnapshot { entry: node.clone(), stats: Some(stats), reachable: true, error: None },
                Err(e) => NodeSnapshot { entry: node.clone(), stats: None, reachable: true, error: Some(e.to_string()) },
            },
            Err(e) => NodeSnapshot { entry: node.clone(), stats: None, reachable: false, error: Some(e.to_string()) },
        };
        snapshots.push(snap);
    }
    ClusterStatus { nodes: snapshots, warnings: Vec::new() }
}

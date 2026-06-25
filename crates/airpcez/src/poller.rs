use airpcez_core::cluster::{ClusterStatus, NodeEntry, NodeSnapshot};
use airpcez_core::model::NodeStats;
use futures_util::future::join_all;
use std::time::Duration;

async fn poll_one(client: &reqwest::Client, node: &NodeEntry) -> NodeSnapshot {
    let url = format!("http://{}/stats", node.addr);
    match client.get(&url).timeout(Duration::from_secs(2)).send().await {
        Ok(resp) => match resp.json::<NodeStats>().await {
            Ok(mut stats) => {
                // Rewrite the rpc_endpoint host with the node's reachable IP.
                if let Some(ep) = stats.rpc_endpoint.as_ref() {
                    if let (Some((host_ip, _)), Some((_, port))) =
                        (node.addr.rsplit_once(':'), ep.rsplit_once(':'))
                    {
                        stats.rpc_endpoint = Some(format!("{host_ip}:{port}"));
                    }
                }
                NodeSnapshot { entry: node.clone(), stats: Some(stats), reachable: true, error: None }
            }
            Err(e) => NodeSnapshot { entry: node.clone(), stats: None, reachable: true, error: Some(e.to_string()) },
        },
        Err(e) => NodeSnapshot { entry: node.clone(), stats: None, reachable: false, error: Some(e.to_string()) },
    }
}

pub async fn poll_nodes(client: &reqwest::Client, nodes: &[NodeEntry]) -> ClusterStatus {
    let futures = nodes.iter().map(|n| poll_one(client, n));
    let snapshots = join_all(futures).await;
    ClusterStatus { nodes: snapshots, warnings: Vec::new() }
}

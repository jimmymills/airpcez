use crate::cluster::NodeEntry;
use serde::{Deserialize, Serialize};

/// A saved launch configuration for a model: launch levers + RPC topology + provenance.
/// SCALAR FIELDS FIRST, `nodes` LAST — TOML requires values before arrays-of-tables.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub model: String,
    #[serde(default)] pub ngl: Option<u32>,
    #[serde(default)] pub tensor_split: Option<String>,
    #[serde(default)] pub main_gpu: Option<u32>,
    #[serde(default)] pub device: Option<String>,
    #[serde(default)] pub cpu_moe: Option<String>,
    #[serde(default)] pub ctx: Option<u32>,
    #[serde(default)] pub no_mmap: bool,
    #[serde(default)] pub flash_attn: Option<String>,
    #[serde(default)] pub threads: Option<u32>,
    #[serde(default)] pub threads_batch: Option<u32>,
    #[serde(default)] pub cache_type_k: Option<String>,
    #[serde(default)] pub cache_type_v: Option<String>,
    #[serde(default)] pub hf_cache_dir: Option<String>,
    #[serde(default)] pub host_label: Option<String>,
    #[serde(default)] pub tok_s: Option<f32>,
    #[serde(default)] pub note: Option<String>,
    #[serde(default)] pub updated_at: u64,
    #[serde(default)] pub nodes: Vec<NodeEntry>,
}

/// Lowercase, collapse runs of non-alphanumerics to a single '-', trim leading/trailing '-'.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("Best Networked"), "best-networked");
        assert_eq!(slugify("solo-2080!!"), "solo-2080");
        assert_eq!(slugify("  A  B  "), "a-b");
        assert_eq!(slugify(""), "");
    }
}

use airpcez_core::planner::ModelMeta;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct CatalogEntry {
    pub label: String,
    pub hf: String,
    pub meta: ModelMeta,
}

/// Approximate Q4_K_M metadata for a few known models. Sizes in MiB.
pub fn model_catalog() -> Vec<CatalogEntry> {
    let e = |label: &str, hf: &str, total_mib: u64, n_layers: u32, is_moe: bool| CatalogEntry {
        label: label.into(),
        hf: hf.into(),
        meta: ModelMeta {
            total_mib,
            n_layers,
            is_moe,
        },
    };
    vec![
        e(
            "Qwen3.6-27B (dense) Q4_K_M",
            "unsloth/Qwen3.6-27B-GGUF:Q4_K_M",
            17_000,
            64,
            false,
        ),
        e(
            "Qwen3.6-35B-A3B (MoE) Q4_K_M",
            "unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M",
            21_000,
            48,
            true,
        ),
        e(
            "Llama-3.3-70B Q4_K_M",
            "unsloth/Llama-3.3-70B-Instruct-GGUF:Q4_K_M",
            42_000,
            80,
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_has_known_models_with_sane_meta() {
        let c = model_catalog();
        assert!(c.len() >= 3);
        let moe = c.iter().find(|e| e.hf.contains("35B-A3B")).unwrap();
        assert!(moe.meta.is_moe && moe.meta.n_layers > 0 && moe.meta.total_mib > 10_000);
        let dense70 = c.iter().find(|e| e.hf.contains("70B")).unwrap();
        assert!(!dense70.meta.is_moe && dense70.meta.total_mib > 35_000);
    }
}

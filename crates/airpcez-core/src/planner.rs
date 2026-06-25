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
}

/// Rough KV-cache size: ~0.125 MiB per layer per 1024 context tokens.
/// A heuristic for the fit check, not an exact figure.
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_json_roundtrips() {
        let p = Plan {
            ngl: 40, tensor_split: Some("12,11".into()), cpu_moe: None,
            exclude_notes: vec!["drop Vulkan0".into()],
            fit: FitVerdict { fit: Fit::Tight, detail: "tight".into() },
            gpu_pool_mib: 21000, cpu_pool_mib: 26000,
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

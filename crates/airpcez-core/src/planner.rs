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
}

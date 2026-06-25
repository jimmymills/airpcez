use airpcez_core::model::Role;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub ui_port: u16,
    pub rpc_port: u16,
    pub llama_port: u16,
    pub role: Role,
    pub llama_dir: Option<String>,
    pub node_name: String,
    #[serde(default)]
    pub nodes: Vec<airpcez_core::cluster::NodeEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ui_port: 8675,
            rpc_port: 50052,
            llama_port: 8080,
            role: Role::Worker,
            llama_dir: None,
            node_name: sysinfo::System::host_name()
                .unwrap_or_else(|| "airpcez-node".to_string()),
            nodes: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
}

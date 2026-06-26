use crate::cluster::NodeEntry;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A saved launch configuration for a model: launch levers + RPC topology + provenance.
/// SCALAR FIELDS FIRST, `nodes` LAST — TOML requires values before arrays-of-tables.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Profile {
    #[serde(default)] pub id: String,
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

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProfileStore {
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

impl ProfileStore {
    /// Missing file → empty store. Garbled file → warn and treat as empty (never panic).
    pub fn load(path: &Path) -> ProfileStore {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!(
                    "[airpcez] WARNING: {} failed to parse ({e}) — treating profiles as empty",
                    path.display()
                );
                ProfileStore::default()
            }),
            Err(_) => ProfileStore::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }

    pub fn list(&self, model: Option<&str>) -> Vec<&Profile> {
        self.profiles
            .iter()
            .filter(|p| match model {
                Some(m) => p.model == m,
                None => true,
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Replace the profile with the same id, else append.
    pub fn upsert(&mut self, p: Profile) {
        if let Some(slot) = self.profiles.iter_mut().find(|x| x.id == p.id) {
            *slot = p;
        } else {
            self.profiles.push(p);
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        self.profiles.len() != before
    }
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

    fn sample(id: &str, model: &str) -> Profile {
        Profile { id: id.into(), name: id.into(), model: model.into(), ..Default::default() }
    }

    #[test]
    fn store_upsert_get_remove() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a", "m1"));
        s.upsert(sample("b", "m2"));
        assert_eq!(s.profiles.len(), 2);
        // upsert replaces same id, does not append
        let mut a2 = sample("a", "m1");
        a2.name = "Renamed".into();
        s.upsert(a2);
        assert_eq!(s.profiles.len(), 2);
        assert_eq!(s.get("a").unwrap().name, "Renamed");
        assert!(s.get("missing").is_none());
        assert!(s.remove("a"));
        assert!(!s.remove("a")); // already gone
        assert_eq!(s.profiles.len(), 1);
    }

    #[test]
    fn store_list_filters_by_model() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a", "m1"));
        s.upsert(sample("b", "m2"));
        s.upsert(sample("c", "m1"));
        assert_eq!(s.list(None).len(), 3);
        let m1: Vec<&str> = s.list(Some("m1")).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(m1, vec!["a", "c"]);
        assert_eq!(s.list(Some("nope")).len(), 0);
    }

    #[test]
    fn store_roundtrips_through_toml_file() {
        let mut s = ProfileStore::default();
        let mut p = sample("best-networked", "unsloth/Q:Q4_K_M");
        p.ngl = Some(99);
        p.cpu_moe = Some("16".into());
        p.tok_s = Some(5.95);
        p.nodes = vec![NodeEntry { name: "m2".into(), addr: "192.168.0.125:8675".into() }];
        s.upsert(p);
        let path = std::env::temp_dir().join("airpcez-profiletest-roundtrip.toml");
        let _ = std::fs::remove_file(&path);
        s.save(&path).unwrap();
        let loaded = ProfileStore::load(&path);
        assert_eq!(loaded.profiles, s.profiles);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn store_load_missing_file_is_empty() {
        let path = std::env::temp_dir().join("airpcez-profiletest-does-not-exist.toml");
        let _ = std::fs::remove_file(&path);
        assert_eq!(ProfileStore::load(&path).profiles.len(), 0);
    }

    #[test]
    fn store_load_garbled_file_is_empty() {
        let path = std::env::temp_dir().join("airpcez-profiletest-garbled.toml");
        std::fs::write(&path, "this is not valid toml !!! [[[").unwrap();
        assert_eq!(ProfileStore::load(&path).profiles.len(), 0);
        let _ = std::fs::remove_file(&path);
    }
}

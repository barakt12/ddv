use std::{collections::HashSet, env, fs, path::PathBuf};

use serde::Deserialize;

/// A DynamoDB key access pattern discovered from the codebase, e.g.
/// name="User connection", pk="ORG#${orgId}#USER#${userId}", sk="CONNECTION#${connectionId}".
#[derive(Debug, Clone, Deserialize)]
pub struct KeyPattern {
    pub name: String,
    pub pk: String,
    #[serde(default)]
    pub sk: String,
}

fn config_home() -> Option<PathBuf> {
    env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
}

/// The shared pattern catalog (generated from the service repos). Overridable
/// with DDV_PATTERNS; defaults to ~/.config/herdr-plus/dynamo-patterns.json.
fn patterns_path() -> Option<PathBuf> {
    if let Ok(p) = env::var("DDV_PATTERNS") {
        return Some(PathBuf::from(p));
    }
    config_home().map(|b| b.join("herdr-plus").join("dynamo-patterns.json"))
}

fn favorites_path() -> Option<PathBuf> {
    config_home().map(|b| b.join("ddv").join("favorites.json"))
}

pub fn load_patterns() -> Vec<KeyPattern> {
    patterns_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Vec<KeyPattern>>(&s).ok())
        .unwrap_or_default()
}

pub fn load_favorites() -> HashSet<String> {
    favorites_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

pub fn save_favorites(favs: &HashSet<String>) {
    if let Some(p) = favorites_path() {
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut v: Vec<&String> = favs.iter().collect();
        v.sort();
        if let Ok(s) = serde_json::to_string_pretty(&v) {
            let _ = fs::write(p, s);
        }
    }
}

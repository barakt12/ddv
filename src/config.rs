use std::{env, path::PathBuf};

use serde::Deserialize;
use smart_default::SmartDefault;
use umbra::optional;

const CONFIG_PATH_ENV_VAR: &str = "DDV_CONFIG";

impl Config {
    pub fn load() -> Config {
        // DDV_CONFIG wins; otherwise fall back to $XDG_CONFIG_HOME/ddv/config.toml
        // (or ~/.config/ddv/config.toml) if it exists.
        let path = env::var(CONFIG_PATH_ENV_VAR)
            .ok()
            .map(PathBuf::from)
            .or_else(default_config_path);
        match path {
            Some(p) if p.is_file() => {
                let content = std::fs::read_to_string(p).unwrap();
                let config: OptionalConfig = toml::from_str(&content).unwrap();
                config.into()
            }
            _ => Config::default(),
        }
    }
}

fn default_config_path() -> Option<PathBuf> {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("ddv").join("config.toml"))
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, SmartDefault)]
pub struct Config {
    #[default = "us-east-1"]
    pub default_region: String,
    /// When non-empty, the startup profile picker shows exactly these profiles
    /// (in order). Empty = show every profile found in the AWS config files.
    #[default(_code = "Vec::new()")]
    pub profiles: Vec<String>,
    #[nested]
    pub ui: UiConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, SmartDefault)]
pub struct UiConfig {
    #[nested]
    pub table_list: UiTableListConfig,
    #[nested]
    pub table: UiTableConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, SmartDefault)]
pub struct UiTableListConfig {
    #[default = 30]
    pub list_width: u16,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, SmartDefault)]
pub struct UiTableConfig {
    #[default = 30]
    pub max_attribute_width: usize,
    #[default = 35]
    pub max_expand_width: u16,
    #[default = 6]
    pub max_expand_height: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_parse_from_toml_and_default_empty() {
        let opt: OptionalConfig = toml::from_str("profiles = [\"local\", \"Admin\"]").unwrap();
        let cfg: Config = opt.into();
        assert_eq!(cfg.profiles, vec!["local".to_string(), "Admin".to_string()]);

        let empty: OptionalConfig = toml::from_str("").unwrap();
        let cfg2: Config = empty.into();
        assert!(cfg2.profiles.is_empty());
    }
}

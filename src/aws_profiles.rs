use std::{collections::BTreeSet, env, fs, path::PathBuf};

/// Enumerate AWS profile names from the shared config and credentials files
/// (honoring `AWS_CONFIG_FILE` / `AWS_SHARED_CREDENTIALS_FILE`), sorted and
/// de-duplicated. Includes `default` and any custom profile such as `local`.
pub fn list_profiles() -> Vec<String> {
    let config_path = env::var("AWS_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| aws_dir().join("config"));
    let creds_path = env::var("AWS_SHARED_CREDENTIALS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| aws_dir().join("credentials"));
    let config = fs::read_to_string(&config_path).unwrap_or_default();
    let creds = fs::read_to_string(&creds_path).unwrap_or_default();
    profiles_from(&config, &creds)
}

/// Extract profile names from config + credentials file contents. Config
/// sections are `[profile name]` / `[default]`; credentials sections are bare
/// `[name]` (with lenient handling of a stray `profile ` prefix).
fn profiles_from(config: &str, creds: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for section in section_headers(config) {
        if section == "default" {
            names.insert("default".to_string());
        } else if let Some(name) = section.strip_prefix("profile ") {
            names.insert(name.trim().to_string());
        }
    }
    for section in section_headers(creds) {
        let name = section.strip_prefix("profile ").unwrap_or(&section);
        names.insert(name.trim().to_string());
    }
    names.into_iter().filter(|n| !n.is_empty()).collect()
}

fn section_headers(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix('[')
                .and_then(|l| l.strip_suffix(']'))
                .map(|inner| inner.trim().to_string())
        })
        .collect()
}

fn aws_dir() -> PathBuf {
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".aws")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_and_credentials() {
        let config = "\
[default]
region = us-east-1

[profile local]
endpoint_url = http://localhost:8000

[profile dev-permissions-1]
sso_session = x
";
        let creds = "\
[default]
aws_access_key_id = a

[dynamodb-docker]
aws_access_key_id = b

[profile barak-staging]
aws_access_key_id = c
";
        let profiles = profiles_from(config, creds);
        assert_eq!(
            profiles,
            vec![
                "barak-staging",     // stray "profile " prefix in creds is stripped
                "default",
                "dev-permissions-1",
                "dynamodb-docker",
                "local",
            ]
        );
    }
}

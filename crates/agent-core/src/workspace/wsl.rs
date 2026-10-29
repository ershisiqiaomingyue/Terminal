use serde::{Deserialize, Serialize};

/// Configuration for a local Windows Subsystem for Linux execution target.
///
/// The target deliberately keeps Linux paths in the agent contract. The local
/// tool adapter maps them to the WSL UNC share only at the filesystem boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WslRuntimeConfig {
    pub distribution: String,
    #[serde(default = "default_wsl_root")]
    pub root: String,
}

fn default_wsl_root() -> String {
    "/".to_string()
}

impl WslRuntimeConfig {
    pub fn normalized(&self) -> Result<Self, String> {
        let distribution = self.distribution.trim().to_string();
        if distribution.is_empty() {
            return Err("WSL 发行版不能为空".to_string());
        }
        if distribution.starts_with('-') || distribution.contains(['\\', '/', '\0']) {
            return Err("WSL 发行版名称包含非法字符".to_string());
        }
        let root = self.root.trim().replace('\\', "/");
        let root = if root.is_empty() { "/" } else { root.as_str() };
        if !root.starts_with('/') || root.contains('\0') {
            return Err("WSL 工作目录必须是 Linux 绝对路径".to_string());
        }
        Ok(Self {
            distribution,
            root: normalize_linux_path(root),
        })
    }
}

pub fn normalize_linux_path(value: &str) -> String {
    let mut result = Vec::new();
    let normalized = value.replace('\\', "/");
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                result.pop();
            }
            value => result.push(value),
        }
    }
    if result.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", result.join("/"))
    }
}

pub fn is_linux_absolute_path(value: &str) -> bool {
    value.trim().starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_wsl_configuration() {
        let config = WslRuntimeConfig {
            distribution: " Ubuntu-22.04 ".to_string(),
            root: "/home/dev/../aosp/".to_string(),
        }
        .normalized()
        .unwrap();
        assert_eq!(config.distribution, "Ubuntu-22.04");
        assert_eq!(config.root, "/home/aosp");
    }

    #[test]
    fn rejects_windows_or_empty_distribution_values() {
        assert!(WslRuntimeConfig {
            distribution: String::new(),
            root: "/".to_string(),
        }
        .normalized()
        .is_err());
        assert!(WslRuntimeConfig {
            distribution: "Ubuntu/22.04".to_string(),
            root: "/".to_string(),
        }
        .normalized()
        .is_err());
        assert!(!is_linux_absolute_path("D:\\workspace"));
    }
}

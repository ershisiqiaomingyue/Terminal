//! Project-owned command/argument entries extend checks without a language list.
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Deserialize)]
pub(crate) struct CheckCommand {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

pub(crate) fn configured_check(cwd: &Path, kind: &str) -> Result<Option<CheckCommand>, String> {
    let path = cwd.join(".agent-checks.json");
    if !path.exists() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    fs::File::open(&path)
        .and_then(|file| file.take(128 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("无法读取项目检查配置：{e}"))?;
    if bytes.len() > 128 * 1024 {
        return Err("项目检查配置超过 128 KiB".into());
    }
    configured_check_bytes(&bytes, kind).map(Some)
}

pub(crate) fn configured_check_bytes(bytes: &[u8], kind: &str) -> Result<CheckCommand, String> {
    if bytes.len() > 128 * 1024 {
        return Err("项目检查配置超过 128 KiB".into());
    }
    #[derive(Deserialize)]
    struct Config {
        checks: BTreeMap<String, CheckCommand>,
    }
    let mut config: Config =
        serde_json::from_slice(&bytes).map_err(|e| format!(".agent-checks.json 格式错误：{e}"))?;
    let entry = config.checks.remove(kind).ok_or_else(|| {
        format!(
            ".agent-checks.json 没有定义 {kind}；可用项：{}",
            config.checks.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    if entry.command.trim().is_empty()
        || entry.command.contains('\0')
        || entry.args.iter().any(|arg| arg.contains('\0'))
        || entry.command.len() > 4096
        || entry.args.len() > 256
        || entry.args.iter().map(String::len).sum::<usize>() > 32 * 1024
    {
        return Err("项目检查命令为空或超过参数限制".into());
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_checks_support_any_toolchain_without_shell_interpolation() {
        let root = std::env::temp_dir().join(format!(
            "project-checks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".agent-checks.json"),r#"{"checks":{"go":{"command":"go","args":["test","./..."]},"python":{"command":"python","args":["-m","pytest"]},"native":{"command":"cmake","args":["--build","build folder"]},"other":{"command":"custom-compiler","args":["check","literal;$value"]}}}"#).unwrap();
        for (kind, command) in [
            ("go", "go"),
            ("python", "python"),
            ("native", "cmake"),
            ("other", "custom-compiler"),
        ] {
            assert_eq!(
                configured_check(&root, kind).unwrap().unwrap().command,
                command
            );
        }
        assert_eq!(
            configured_check(&root, "native").unwrap().unwrap().args[1],
            "build folder"
        );
        assert_eq!(
            configured_check(&root, "other").unwrap().unwrap().args[1],
            "literal;$value"
        );
        assert!(configured_check(&root, "missing")
            .unwrap_err()
            .contains("可用项"));
        fs::remove_dir_all(root).unwrap();
    }
}

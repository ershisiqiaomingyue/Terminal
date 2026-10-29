//! Shared runtime resolution and process environment for every plugin capability.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) struct PluginEnvironments {
    executables: BTreeMap<String, PathBuf>,
}

impl PluginEnvironments {
    /// Resolve only declared dependencies. Missing runtimes are installed by the
    /// environment UI after confirmation, never privately by a plugin launcher.
    pub(crate) fn resolve(required: &[&str], legacy_node: Option<&Path>) -> Result<Self, String> {
        Self::resolve_with(required, |id| match id {
            "node" => {
                let result = crate::android_tools::inspect_node_runtime();
                if result.is_ok() {
                    return result;
                }
                // Adopt a validated runtime left by the earlier model-plugin installer.
                if let Some(path) = legacy_node.filter(|path| path.is_file()) {
                    if crate::android_tools::validate_node_runtime(path).is_ok() {
                        crate::android_tools::save_environment_tool_path(
                            crate::android_tools::EnvironmentToolPathRequest {
                                tool_id: "node".into(),
                                path: path.to_string_lossy().into_owned(),
                            },
                        )?;
                        return Ok(path.to_path_buf());
                    }
                }
                result
            }
            "python" => crate::android_tools::inspect_python_runtime().map(|(path, _)| path),
            _ => Err(format!("不支持的插件运行环境：{id}")),
        })
    }

    fn resolve_with(
        required: &[&str],
        mut inspect: impl FnMut(&str) -> Result<PathBuf, String>,
    ) -> Result<Self, String> {
        if let Some(id) = required
            .iter()
            .find(|id| !matches!(**id, "node" | "python"))
        {
            return Err(format!("不支持的插件运行环境：{id}"));
        }
        let mut executables = BTreeMap::new();
        for id in required {
            if !executables.contains_key(*id) {
                executables.insert((*id).to_string(), inspect(id)?);
            }
        }
        Ok(Self { executables })
    }

    pub(crate) fn executable(&self, id: &str) -> Result<&Path, String> {
        self.executables
            .get(id)
            .map(PathBuf::as_path)
            .ok_or_else(|| format!("插件未声明 {id} 运行环境"))
    }

    /// Absolute interpreter for the main process; shared tools also precede the
    /// system PATH so grandchildren use the same portable environment.
    pub(crate) fn command(&self, id: &str) -> Result<Command, String> {
        let mut command = crate::background_process::command(self.executable(id)?);
        agent_core::apply_child_environment_std(&mut command);
        self.apply(&mut command)?;
        Ok(command)
    }

    fn apply(&self, command: &mut Command) -> Result<(), String> {
        let mut directories = Vec::new();
        for (id, executable) in &self.executables {
            command.env(
                format!("ANDROID_TOOL_{}", id.to_ascii_uppercase()),
                executable,
            );
            if let Some(parent) = executable.parent() {
                directories.push(parent.to_path_buf());
                if id == "python" && parent.join("Scripts").is_dir() {
                    directories.push(parent.join("Scripts"));
                }
            }
        }
        let existing = command
            .get_envs()
            .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
            .and_then(|(_, value)| value.map(|value| value.to_os_string()))
            .or_else(|| std::env::var_os("PATH"));
        if let Some(existing) = existing {
            directories.extend(std::env::split_paths(&existing));
        }
        let mut unique = Vec::new();
        for directory in directories {
            if !unique.contains(&directory) {
                unique.push(directory);
            }
        }
        let path =
            std::env::join_paths(unique).map_err(|error| format!("插件环境 PATH 无效：{error}"))?;
        command.env("PATH", path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependencies_are_deduplicated_and_failure_blocks_launch() {
        let mut inspected = Vec::new();
        let result = PluginEnvironments::resolve_with(&["node", "node", "python"], |id| {
            inspected.push(id.to_string());
            if id == "python" {
                Err("Python unavailable".into())
            } else {
                Ok(PathBuf::from("portable/node"))
            }
        });
        assert!(result.is_err());
        assert_eq!(inspected, ["node", "python"]);
        assert!(PluginEnvironments::resolve_with(&["unknown"], |_| panic!(
            "must validate before resolving"
        ))
        .is_err());
    }

    #[test]
    fn command_uses_selected_interpreter_and_preserves_other_tools() {
        let root = std::env::temp_dir().join("android-tool-plugin-environment");
        let node = root.join("node").join("node.exe");
        let python = root.join("python").join("python.exe");
        let environments = PluginEnvironments::resolve_with(&["node", "python"], |id| {
            Ok(if id == "node" {
                node.clone()
            } else {
                python.clone()
            })
        })
        .unwrap();
        let command = environments.command("node").unwrap();
        assert_eq!(command.get_program(), node.as_os_str());
        let vars: BTreeMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect();
        assert_eq!(
            vars[std::ffi::OsStr::new("ANDROID_TOOL_PYTHON")],
            python.as_os_str()
        );
        let paths: Vec<_> = std::env::split_paths(vars[std::ffi::OsStr::new("PATH")]).collect();
        assert_eq!(paths[0], node.parent().unwrap());
        assert_eq!(paths[1], python.parent().unwrap());
        assert!(environments.command("undeclared").is_err());
        let mut child = Command::new(&node);
        let other_tool = root.join("other-tool");
        child.env(
            "PATH",
            std::env::join_paths([other_tool.clone(), node.parent().unwrap().to_path_buf()])
                .unwrap(),
        );
        environments.apply(&mut child).unwrap();
        let path = child
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .unwrap()
            .1
            .unwrap();
        assert_eq!(
            std::env::split_paths(path).collect::<Vec<_>>(),
            [
                node.parent().unwrap().to_path_buf(),
                python.parent().unwrap().to_path_buf(),
                other_tool
            ]
        );
    }
}

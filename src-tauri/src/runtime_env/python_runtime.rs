use super::*;

const PYTHON_PACKAGE_INDEX: &str = "https://api.nuget.org/v3-flatcontainer/python/index.json";

#[derive(Deserialize)]
struct PythonPackageIndex {
    versions: Vec<String>,
}

// CPython's NuGet distribution includes pip and the complete standard library.
// Keep the maintained 3.13 line for compatibility with third-party skill packages.
fn latest_python_package(versions: &[String]) -> Option<String> {
    versions
        .iter()
        .filter_map(|version| {
            let parts = version
                .split('.')
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            (parts.len() == 3 && parts[0] == 3 && parts[1] == 13).then(|| (parts[2], version))
        })
        .max_by_key(|(patch, _)| *patch)
        .map(|(_, version)| version.clone())
}

pub(super) async fn python_environment_tool_release() -> Result<EnvironmentToolRemote, String> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        return Err("便携 Python 下载目前支持 Windows x64，请选择本地 Python。".into());
    }
    let index = environment_http_client()?
        .get(PYTHON_PACKAGE_INDEX)
        .send()
        .await
        .map_err(|error| format!("无法查询 Python 版本：{error}"))?
        .error_for_status()
        .map_err(|error| format!("Python 版本请求失败：{error}"))?
        .json::<PythonPackageIndex>()
        .await
        .map_err(|error| format!("无法读取 Python 版本：{error}"))?;
    let version = latest_python_package(&index.versions).ok_or("没有可用的稳定版 Python 3.13")?;
    Ok(EnvironmentToolRemote {
        url: format!(
            "https://api.nuget.org/v3-flatcontainer/python/{version}/python.{version}.nupkg"
        ),
        file_name: format!("python.{version}.nupkg"),
        version,
        package_kind: EnvironmentToolPackageKind::Zip,
    })
}

pub(crate) fn find_python() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("python") {
        return Some(path);
    }
    let candidates = if cfg!(windows) {
        ["python.exe", "python3.exe"]
    } else {
        ["python3", "python"]
    };
    for executable in candidates {
        let mut command = background_command(if cfg!(windows) { "where.exe" } else { "which" });
        command.arg(executable);
        let Ok(output) = command_output_with_timeout(command, Duration::from_secs(3)) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for path in decode_environment_tool_output(&output.stdout)
            .lines()
            .map(str::trim)
        {
            // Windows Store execution aliases are launchers, not an installed runtime.
            if cfg!(windows)
                && path
                    .replace('\\', "/")
                    .to_ascii_lowercase()
                    .contains("/microsoft/windowsapps/")
            {
                continue;
            }
            let path = PathBuf::from(path);
            if path.is_file() {
                return Some(normalized_environment_tool_path(&path));
            }
        }
    }
    if cfg!(windows) {
        let mut command = background_command("py");
        command.args(["-3", "-I", "-c", "import sys; print(sys.executable)"]);
        let output = command_output_with_timeout(command, Duration::from_secs(5)).ok()?;
        if !output.status.success() {
            return None;
        }
        let path = PathBuf::from(decode_environment_tool_output(&output.stdout).trim());
        return path
            .is_file()
            .then(|| normalized_environment_tool_path(&path));
    }
    None
}

pub(crate) fn inspect_python_runtime() -> Result<(PathBuf, String), String> {
    let path = find_python().ok_or("未检测到 Python，请在环境中下载或选择本地 Python。")?;
    let version = environment_tool_version("python", &path)?.ok_or("Python 没有返回版本信息")?;
    Ok((path, version))
}

pub(super) fn python_child_directories(executable: &Path) -> Vec<PathBuf> {
    let Some(root) = executable.parent() else {
        return Vec::new();
    };
    let mut directories = vec![root.to_path_buf()];
    let scripts = root.join("Scripts");
    if scripts.is_dir() {
        directories.push(scripts);
    }
    directories
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_release_uses_stable_patch_order_and_rejects_preview_versions() {
        let versions = [
            "3.13.9",
            "3.13.10",
            "3.13.11-rc1",
            "3.14.0",
            "3.13.100.1",
            "bad",
            "3",
            "3.13",
        ]
        .map(String::from);
        assert_eq!(latest_python_package(&versions).as_deref(), Some("3.13.10"));
        assert!(latest_python_package(&["3.13.0-b1".into()]).is_none());
    }

    #[test]
    fn python_environment_includes_interpreter_and_installed_script_commands() {
        let root = std::env::temp_dir().join(format!("python-env-{}", std::process::id()));
        fs::create_dir_all(root.join("Scripts")).unwrap();
        let path = root.join("python.exe");
        assert_eq!(
            python_child_directories(&path),
            vec![root.clone(), root.join("Scripts")]
        );
        assert!(known_environment_tool("python"));
        assert!(environment_tool_path_is_valid("python", &path));
        assert!(!environment_tool_path_is_valid(
            "python",
            &root.join("py.exe")
        ));
        assert_eq!(
            downloaded_environment_tool_file_name("python"),
            if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn python_package_install_preserves_standard_library_and_pip() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("python-package-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let archive_path = root.join("python.tmp");
        let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        for (name, content) in [
            ("tools/python.exe", "interpreter"),
            ("tools/Lib/json/__init__.py", "standard library"),
            ("tools/Lib/site-packages/pip/__main__.py", "pip"),
            ("tools/Scripts/pip.exe", "pip command"),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
        let installed = install_downloaded_environment_tool(
            &root,
            "python",
            &EnvironmentToolRemote {
                version: "3.13.10".into(),
                url: String::new(),
                file_name: "python.3.13.10.nupkg".into(),
                package_kind: EnvironmentToolPackageKind::Zip,
            },
            &archive_path,
        )
        .unwrap();
        let tools = root.join("python-3.13.10/tools");
        assert_eq!(installed, tools.join("python.exe"));
        assert_eq!(
            python_child_directories(&installed),
            vec![tools.clone(), tools.join("Scripts")]
        );
        assert!(tools.join("Lib/json/__init__.py").is_file());
        assert!(tools.join("Lib/site-packages/pip/__main__.py").is_file());
        assert!(!archive_path.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

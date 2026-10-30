use super::*;

const NODE_RUNTIME_VERSION: &str = "24.14.1";

pub(crate) fn node_version_supported(version: &str) -> bool {
    let Ok(parts) = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    matches!(parts.as_slice(), [22, minor, _] if *minor >= 19)
        || matches!(parts.as_slice(), [major, _, _] if *major >= 24)
}

pub(super) fn find_node() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("node") {
        return Some(path);
    }
    let mut command = background_command("node");
    command.args(["--print", "process.execPath"]);
    let output = command_output_with_timeout(command, Duration::from_secs(5)).ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(decode_environment_tool_output(&output.stdout).trim());
    path.is_file()
        .then(|| normalized_environment_tool_path(&path))
}

pub(crate) fn node_npm_cli(node: &Path) -> Option<PathBuf> {
    let parent = node.parent()?;
    [
        parent.join("node_modules/npm/bin/npm-cli.js"),
        parent.join("../lib/node_modules/npm/bin/npm-cli.js"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub(crate) fn inspect_node_runtime() -> Result<PathBuf, String> {
    let node = find_node().ok_or("未找到 Node.js，请在设置 → 环境中安装或选择 Node.js。")?;
    validate_node_runtime(&node)?;
    Ok(node)
}

pub(crate) fn validate_node_runtime(node: &Path) -> Result<(), String> {
    environment_tool_version("node", node)?;
    Ok(())
}

pub(super) fn validate_node_npm(node: &Path) -> Result<(), String> {
    let npm = node_npm_cli(node).ok_or("Node.js 缺少 npm，请在环境中下载完整便携包。")?;
    let mut command = background_command(node);
    command.arg(npm).arg("--version");
    let output = command_output_with_timeout_limit(command, Duration::from_secs(12), 128 * 1024)?;
    if !output.status.success()
        || decode_environment_tool_output(&output.stdout)
            .trim()
            .is_empty()
    {
        return Err("Node.js 的 npm 无法运行，请在环境中下载完整便携包。".into());
    }
    Ok(())
}

pub(super) async fn node_environment_tool_release() -> Result<EnvironmentToolRemote, String> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        return Err("便携 Node 下载目前支持 Windows x64，请选择本地 Node.js。".into());
    }
    let file_name = format!("node-v{NODE_RUNTIME_VERSION}-win-x64.zip");
    Ok(EnvironmentToolRemote {
        version: NODE_RUNTIME_VERSION.to_string(),
        url: format!("https://nodejs.org/dist/v{NODE_RUNTIME_VERSION}/{file_name}"),
        file_name,
        package_kind: EnvironmentToolPackageKind::Zip,
    })
}

pub(super) async fn node_download_checksum(
    remote: &EnvironmentToolRemote,
) -> Result<String, String> {
    let base = remote.url.rsplit_once('/').ok_or("Node 下载地址无效")?.0;
    let response = environment_http_client()?
        .get(format!("{base}/SHASUMS256.txt"))
        .send()
        .await
        .map_err(|error| format!("无法获取 Node 官方校验值：{error}"))?
        .error_for_status()
        .map_err(|error| format!("Node 官方校验请求失败：{error}"))?;
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("无法读取 Node 校验值：{error}"))?;
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err("Node 校验信息超过大小限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let content = String::from_utf8(bytes).map_err(|_| "Node 官方校验信息格式无效")?;
    checksum_for_archive(&content, &remote.file_name).ok_or("缺少 Node 官方 SHA-256 校验值".into())
}

fn checksum_for_archive(content: &str, filename: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?;
        (name == filename
            && fields.next().is_none()
            && hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

pub(super) fn verify_node_archive(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| format!("无法读取 Node 下载文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法校验 Node 下载文件：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != expected {
        // A corrupt partial download must not poison the next retry.
        drop(file);
        let _ = fs::remove_file(path);
        return Err("Node 下载 SHA-256 校验失败，请重试；插件未安装。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_node_lines_only() {
        for version in ["v22.19.0", "22.22.1", "v24.14.1", "v26.0.0"] {
            assert!(node_version_supported(version));
        }
        for version in [
            "v22.18.0",
            "v20.0.0",
            "v23.1.0",
            "v24",
            "invalid",
            "24.bad.1.0",
        ] {
            assert!(!node_version_supported(version));
        }
    }

    #[test]
    fn checksum_requires_exact_filename_and_valid_hash() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum_for_archive(&format!("{hash}  node.zip\n"), "node.zip"),
            Some(hash)
        );
        assert!(checksum_for_archive("invalid  node.zip", "node.zip").is_none());
        assert!(
            checksum_for_archive(&format!("{}  other.zip", "a".repeat(64)), "node.zip").is_none()
        );
    }

    #[test]
    fn corrupted_archive_is_removed_before_retry() {
        let path = std::env::temp_dir().join(format!(
            "node-checksum-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::write(&path, b"corrupt archive").unwrap();
        assert!(verify_node_archive(&path, &"0".repeat(64)).is_err());
        assert!(!path.exists());
    }
}

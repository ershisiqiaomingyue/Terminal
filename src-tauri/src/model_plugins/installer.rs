use super::*;
use std::{
    path::Path,
    process::Command,
    time::Duration,
};

fn runtime_root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|p| p.join("model-plugins"))
        .map_err(|e| e.to_string())
}
fn progress(message: &str) {
    if let Ok(mut current) = PROGRESS.lock() {
        *current = message.into();
    }
}
/// Resolves the runtime the plugin declared, so nothing here names an
/// interpreter: a Node provider installs through npm and a Python one through
/// pip with the same call.
pub(super) fn pipeline(
    app: &AppHandle,
    id: &str,
) -> Result<crate::plugin_runtime::Pipeline, String> {
    let legacy_node = runtime_root(app)?.join("runtime").join("node.exe");
    crate::plugin_runtime::Pipeline::resolve(required_environment_tools(id), Some(&legacy_node))
}
fn download(url: &str, max_bytes: usize, timeout: u64) -> Result<Vec<u8>, String> {
    tauri::async_runtime::block_on(async {
        let client = agent_core::http_client_builder()?
            .timeout(Duration::from_secs(timeout))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("插件下载失败：{e}"))?
            .error_for_status()
            .map_err(|e| format!("插件下载失败：{e}"))?;
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err("插件下载文件过大".into());
        }
        use futures_util::StreamExt;
        let mut chunks = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            if bytes.len() + chunk.len() > max_bytes {
                return Err("插件下载文件过大".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
}
/// Where a provider's published version can be asked for, if anywhere.
///
/// npm publishes one global registry, so a Node provider can be asked for its
/// latest version. A Python provider cannot: its index is whatever the user's
/// pip is configured for (mirrors included), so the version declared in the spec
/// stays authoritative and no request is made.
fn published_version_url(
    language: crate::plugin_runtime::Language,
    package: &str,
) -> Option<String> {
    match language {
        crate::plugin_runtime::Language::Node => {
            Some(format!("https://registry.npmjs.org/{package}/latest"))
        }
        crate::plugin_runtime::Language::Python => None,
    }
}
pub(super) fn latest_version(id: &str) -> Result<String, String> {
    let spec = super::spec::require(id)?;
    let language = crate::plugin_runtime::Language::primary(spec.environment_tools)?;
    let Some(url) = published_version_url(language, spec.package) else {
        return Ok(spec.base_version.into());
    };
    let document: serde_json::Value = serde_json::from_slice(&download(&url, 1024 * 1024, 20)?)
    .map_err(|e| e.to_string())?;
    let version = document["version"].as_str().ok_or("插件版本信息缺失")?;
    let parts: Vec<_> = version
        .split('.')
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();
    // Only update within the adapter line whose exported API was validated.
    Ok(if spec.accepts(&parts) {
        version.into()
    } else {
        spec.base_version.into()
    })
}
/// One pipeline step, reported with the model plugin wording.
fn run(command: Command, directory: &Path, timeout: Duration) -> Result<(), String> {
    crate::plugin_runtime::run(command, directory, timeout, 8, |_, _| {}).map_err(|failure| {
        match failure {
            crate::plugin_runtime::Failure::Spawn(error) => format!("无法执行插件安装：{error}"),
            crate::plugin_runtime::Failure::TimedOut => {
                "插件安装超时，原有插件已保留，请稍后重试".into()
            }
            crate::plugin_runtime::Failure::Exit { tail } => {
                format!("插件安装失败，请检查网络或重试：{tail}")
            }
        }
    })
}

pub(super) fn install(app: &AppHandle, id: &str) -> Result<(String, String), String> {
    progress("正在检查插件版本与运行环境…");
    let spec = super::spec::require(id)?;
    let pipeline = pipeline(app, id)?;
    let version = latest_version(id).unwrap_or_else(|_| spec.base_version.into());
    let name = chrono::Utc::now().timestamp_millis().to_string();
    // Install into private app data; lifecycle scripts are never run.
    let directory =
        crate::plugin_runtime::prepare_private_dir(&root(app, id)?, &name, pipeline.language())?
            .directory;
    let result = (|| {
        progress("正在下载模型连接插件及依赖…");
        // One generic install: the plugin's own package manager, picked from the
        // declared runtime, resolves its full dependency and peer graph inside
        // this private directory. Nothing is pinned per plugin here, so a
        // provider added later cannot be broken by a stale hardcoded peer list.
        run(
            pipeline.install_command(
                &directory,
                &[format!("{}@{version}", spec.package)],
                crate::plugin_runtime::InstallOptions::default(),
            )?,
            &directory,
            Duration::from_secs(300),
        )?;
        progress("正在验证插件接口…");
        run(
            pipeline.verification_command(&directory, spec.package, spec.required_exports)?,
            &directory,
            Duration::from_secs(30),
        )?;
        progress("正在启动插件并同步模型…");
        Ok((name, version))
    })();
    if result.is_err() {
        crate::plugin_runtime::remove_failed(&directory);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::Language;

    #[test]
    fn node_providers_ask_npm_and_python_providers_ask_nothing() {
        assert_eq!(
            published_version_url(Language::Node, "dsh-opencode-go").as_deref(),
            Some("https://registry.npmjs.org/dsh-opencode-go/latest")
        );
        // A Python index belongs to the user's pip configuration, so asking it
        // would let the answer drift away from the audited plugin version.
        assert_eq!(published_version_url(Language::Python, "dsh-opencode-go"), None);
    }

    #[test]
    fn shipped_providers_keep_the_node_source() {
        for id in crate::model_plugins::spec::ids() {
            let spec = crate::model_plugins::spec::require(id).expect("spec row");
            assert_eq!(
                Language::primary(spec.environment_tools),
                Ok(Language::Node),
                "{id} changed package manager, which also changes where its version comes from"
            );
        }
    }

    /// Manual end-to-end run of the two real steps in `install` (install into a
    /// private directory, then verify the exports) without an app handle. This is
    /// the check that a published provider really arrives with its whole
    /// dependency graph. Run with
    /// `cargo test --lib published_provider -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs network and a local Node runtime"]
    fn published_provider_installs_with_all_dependencies() {
        let id = std::env::var("PROBE_PROVIDER").unwrap_or_else(|_| "opencode".into());
        let spec = crate::model_plugins::spec::require(&id).expect("known provider");
        let pipeline =
            crate::plugin_runtime::Pipeline::resolve(spec.environment_tools, None).expect("runtime");
        let root = std::env::temp_dir().join(format!(
            "plugin-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        let directory =
            crate::plugin_runtime::prepare_private_dir(&root, "1700000000000", pipeline.language())
                .expect("private dir")
                .directory;
        let package = format!("{}@{}", spec.package, spec.base_version);
        let command = pipeline
            .install_command(
                &directory,
                &[package.clone()],
                crate::plugin_runtime::InstallOptions {
                    local_links: false,
                    observe: true,
                },
            )
            .expect("install command");
        let install = crate::plugin_runtime::run(
            command,
            &directory,
            Duration::from_secs(300),
            8,
            |_, _| {},
        );
        let verification = pipeline
            .verification_command(&directory, spec.package, spec.required_exports)
            .and_then(|command| {
                crate::plugin_runtime::run(
                    command,
                    &directory,
                    Duration::from_secs(60),
                    8,
                    |_, _| {},
                )
                .map_err(|failure| format!("{failure:?}"))
            });
        // A peer that a stale hardcoded host list used to omit; overridable so the
        // probe is not tied to one provider's declaration.
        let peer_name =
            std::env::var("PROBE_PEER").unwrap_or_else(|_| "dsh-launch-environment".into());
        let peer = directory
            .join("node_modules")
            .join("@deepseek-ai")
            .join(&peer_name)
            .is_dir();
        let modules = fs::read_dir(directory.join("node_modules"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        println!(
            "provider={id} package={package} install={install:?} verification={verification:?} topLevelModules={modules} peer={peer_name}:{peer}"
        );
        let _ = fs::remove_dir_all(&root);
        assert!(install.is_ok(), "{install:?}");
        assert!(verification.is_ok(), "{verification:?}");
        assert!(modules > 0 && peer);
    }
}

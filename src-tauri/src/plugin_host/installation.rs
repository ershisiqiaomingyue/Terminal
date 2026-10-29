use super::*;
mod local;
use crate::plugin_runtime::{Failure, InstallOptions, Pipeline};
pub(super) use local::upgrade as upgrade_local;
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command, sync::OnceLock, time::Duration};

/// 宿主依赖清单的唯一数据来源：条目既决定 npm 安装参数，也参与 host-revision /
/// local-graph-revision 摘要。分类与「为什么必须 pin」写在各分类的 note 里，
/// 逐条实测依据见 .cache/plugin-verify/task6-section.md 第 11 章。
const HOST_DEPENDENCIES_JSON: &str = include_str!("host-dependencies.json");

/// 摘要输入里的伴生脚本：prepare_runtime 写出的 6 个宿主脚本，加上本机插件图脚本
/// local-packages.mjs（后者原先只参与 local-graph-revision，纳入同一份摘要后
/// 「脚本变了要重建」的不变式对两处都成立）。
const HOST_SCRIPTS: &[&str] = &[
    include_str!("install.mjs"),
    include_str!("server.mjs"),
    include_str!("events.mjs"),
    include_str!("services.mjs"),
    include_str!("model-providers.mjs"),
    include_str!("client-host.mjs"),
    include_str!("local-packages.mjs"),
];

/// host-revision（宿主脚本是否要重写、依赖是否要补装）与 local-graph-revision
/// （本机插件图是否要重建）共用的摘要：7 个伴生脚本 + 依赖清单**展开后的 specifier 序列**。
/// 刻意不摘要 JSON 原文：清单里的 note 是写给人看的理由，改文档不该让每个已装插件重写脚本、
/// 让本机插件重建一次图；真正影响安装结果的只有脚本与 specifier。清单解析失败时安装本身会
/// 带着错误信息失败，摘要值此时无意义。两处调用同一个函数，喂给 sha256 的字节序列必然是同一份，
/// 否则本机插件会反复重建图。
fn host_revision() -> String {
    let mut digest = Sha256::new();
    for script in HOST_SCRIPTS {
        digest.update(script.as_bytes());
    }
    for specifier in host_dependencies().unwrap_or_default() {
        digest.update(specifier.as_bytes());
        // 分隔符：避免 [a@1, b@2] 与 [a@1b@2] 摘要相同。
        digest.update(b"\n");
    }
    format!("{:x}", digest.finalize())
}

/// 按 JSON 里 `packages` 的书写顺序展开成安装参数（顺序 = 安装参数顺序，由守卫测试冻结）；
/// 解析只做一次（每个插件都会走到这里）。`categories` 只承载分类理由，但条目引用的分类必须
/// 在其中，免得分类与理由悄悄脱节。
fn host_dependencies() -> Result<&'static [String], String> {
    static CACHE: OnceLock<Result<Vec<String>, String>> = OnceLock::new();
    match CACHE.get_or_init(|| parse_host_dependencies(HOST_DEPENDENCIES_JSON)) {
        Ok(packages) => Ok(packages),
        Err(error) => Err(error.clone()),
    }
}

fn parse_host_dependencies(text: &str) -> Result<Vec<String>, String> {
    let manifest: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("宿主依赖清单不是合法 JSON：{error}"))?;
    let known: Vec<String> = manifest["categories"]
        .as_array()
        .ok_or("宿主依赖清单缺少 categories")?
        .iter()
        .map(|category| {
            category["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "宿主依赖清单的分类必须有字符串 id".to_string())
        })
        .collect::<Result<_, _>>()?;
    let mut packages = Vec::new();
    for entry in manifest["packages"]
        .as_array()
        .ok_or("宿主依赖清单缺少 packages")?
    {
        let category = entry["category"]
            .as_str()
            .ok_or("宿主依赖清单条目必须有 category")?;
        if !known.iter().any(|id| id == category) {
            return Err(format!("宿主依赖清单条目引用了未知分类：{category}"));
        }
        packages.push(
            entry["specifier"]
                .as_str()
                .ok_or("宿主依赖清单条目必须有字符串 specifier")?
                .to_string(),
        );
    }
    if packages.is_empty() {
        return Err("宿主依赖清单为空".into());
    }
    Ok(packages)
}
/// 宿主插件的脚本（install.mjs / server.mjs / local-packages.mjs）都是 .mjs，必须由 Node
/// 运行，因此把 node 放在声明列表首位：共享流水线按第一个声明选主运行时，其余声明一并解析，
/// 与旧实现 PluginEnvironments::resolve + command("node") 就绪的运行时集合逐字等价。
pub(super) fn host_pipeline(required: &[String]) -> Result<Pipeline, String> {
    let mut declared = vec!["node"];
    declared.extend(required.iter().map(String::as_str));
    Pipeline::resolve(&declared, None)
}

pub(super) fn declared_environments(pkg: &serde_json::Value) -> Result<Vec<String>, String> {
    let mut required = vec!["node".to_string()];
    if let Some(dependencies) = pkg["androidTool"]["requiredEnvironmentTools"].as_array() {
        for dependency in dependencies {
            let id = dependency.as_str().ok_or("插件环境声明无效")?;
            if !matches!(id, "node" | "python") {
                return Err(format!("插件声明了未支持的运行环境：{id}"));
            }
            if !required.iter().any(|value| value == id) {
                required.push(id.into());
            }
        }
    }
    Ok(required)
}
pub(super) fn prepare_runtime(
    path: &Path,
    pipeline: &Pipeline,
    package_name: &str,
) -> Result<(), String> {
    let files = [
        ("install.mjs", include_str!("install.mjs")),
        ("server.mjs", include_str!("server.mjs")),
        ("events.mjs", include_str!("events.mjs")),
        ("services.mjs", include_str!("services.mjs")),
        ("model-providers.mjs", include_str!("model-providers.mjs")),
        ("client-host.mjs", include_str!("client-host.mjs")),
    ];
    let signature = host_revision();
    if fs::read_to_string(path.join("host-revision"))
        .ok()
        .as_deref()
        == Some(&signature)
        && path.join("client-host.js").is_file()
        && path.join("descriptor.json").is_file()
    {
        return Ok(());
    }
    for (file, content) in files {
        fs::write(path.join(file), content).map_err(|e| e.to_string())?;
    }
    // A new host revision must also upgrade an existing plugin's host services.
    let missing = host_dependencies()?.iter().any(|spec| {
        let (name, version) = spec.rsplit_once('@').unwrap();
        fs::read(path.join("node_modules").join(name).join("package.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_none_or(|pkg| pkg["version"].as_str() != Some(version))
    });
    if missing {
        if path.join("local-plugin.json").is_file() {
            return Err(
                "本机插件的宿主依赖需要更新，请从本机插件列表重新加载；不会重新下载安装本机插件"
                    .into(),
            );
        }
        run(
            pipeline.install_command(
                path,
                host_dependencies()?,
                InstallOptions {
                    local_links: true,
                    observe: false,
                },
            )?,
            path,
            Duration::from_secs(300),
            None,
        )?;
    }
    let mut command = pipeline.launch_command(path, &path.join("install.mjs"))?;
    command.arg(path).arg(package_name);
    run(command, path, Duration::from_secs(60), None)?;
    fs::write(path.join("host-revision"), signature).map_err(|e| e.to_string())
}
fn run(
    command: Command,
    directory: &Path,
    timeout: Duration,
    mut progress: Option<&mut progress::InstallProgress<'_>>,
) -> Result<(), String> {
    crate::plugin_runtime::run(command, directory, timeout, 12, |dir, last| {
        if let Some(progress) = progress.as_deref_mut() {
            progress.poll(dir, last);
        }
    })
    .map_err(|failure| match failure {
        Failure::Spawn(error) => format!("无法执行插件安装：{error}"),
        Failure::TimedOut => "插件安装超时".into(),
        Failure::Exit { tail } => format!("插件安装失败：{tail}"),
    })
}
pub(super) fn install(
    app: &AppHandle,
    source: &str,
    required: &[String],
    progress: &mut progress::InstallProgress<'_>,
) -> Result<Plugin, String> {
    progress.phase("preparing");
    let source = source.trim();
    if source.is_empty() {
        return Err("请填写插件包名或选择本地插件目录".into());
    }
    let original_source = source;
    let archive = if Path::new(source).is_file() {
        progress.phase("extracting");
        Some(super::archive::Package::open(Path::new(source))?)
    } else {
        None
    };
    let extracted_source = archive
        .as_ref()
        .map(|package| package.root.to_string_lossy().into_owned());
    let source = extracted_source.as_deref().unwrap_or(source);
    let local = Path::new(source).is_dir();
    if !local
        && !regex::Regex::new(r"^(@[a-z0-9._-]+/)?[a-z0-9._-]+(@[a-zA-Z0-9._+-]+)?$")
            .unwrap()
            .is_match(source)
    {
        return Err("请填写 npm 插件包名（可带版本），或选择本地插件目录".into());
    }
    let (package_name, requirements) = if local {
        let pkg: serde_json::Value = serde_json::from_slice(
            &fs::read(Path::new(source).join("package.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        (
            pkg["name"].as_str().ok_or("插件包名称缺失")?.to_string(),
            declared_environments(&pkg)?,
        )
    } else {
        let at = source.rfind('@').filter(|i| *i > 0);
        (
            at.map(|i| &source[..i]).unwrap_or(source).to_string(),
            required.to_vec(),
        )
    };
    let pipeline = host_pipeline(&requirements)?;
    let id = format!("{:x}", Sha256::digest(package_name.as_bytes()))[..16].to_string();
    let base = directory(app, &id)?;
    let name = chrono::Utc::now().timestamp_millis().to_string();
    let path = base.join("versions").join(&name);
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    let result = (|| {
        fs::write(
            path.join("package.json"),
            "{\"private\":true,\"type\":\"module\"}",
        )
        .map_err(|e| e.to_string())?;
        if local {
            local::install(app, source, &path, &pipeline, progress)?;
        } else {
            let mut packages = host_dependencies()?.to_vec();
            packages.push(source.to_string());
            let command = pipeline.install_command(
                &path,
                &packages,
                InstallOptions {
                    local_links: true,
                    observe: true,
                },
            )?;
            progress.phase("downloading");
            run(command, &path, Duration::from_secs(300), Some(progress))?;
        }
        progress.phase("verifying");
        prepare_runtime(&path, &pipeline, &package_name)?;
        let descriptor: serde_json::Value = serde_json::from_slice(
            &fs::read(path.join("descriptor.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let requirements = declared_environments(
            &serde_json::json!({"androidTool": {"requiredEnvironmentTools": descriptor["requiredEnvironmentTools"]}}),
        )?;
        crate::plugin_environments::PluginEnvironments::resolve(
            &requirements.iter().map(String::as_str).collect::<Vec<_>>(),
            None,
        )?;
        Ok(Plugin {
            id: id.clone(),
            name: package_name,
            source: original_source.into(),
            version: descriptor["version"].as_str().unwrap_or_default().into(),
            description: descriptor["description"]
                .as_str()
                .unwrap_or_default()
                .into(),
            enabled: true,
            required_environment_tools: requirements,
            install_directory: name,
            status: String::new(),
            message: String::new(),
            frame_url: None,
            bridge_key: None,
            settings_inline: descriptor["settingsInline"].as_bool().unwrap_or(false),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 展开后的安装顺序（= JSON 里 `packages` 的书写顺序）冻结在这里：任何重排、增删都会让
    /// 断言失败，因为顺序变化会同时改变 npm 参数与 host-revision。要改清单请先改这里并说明原因。
    const FROZEN_ORDER: &[&str] = &[
        // 改前 installation.rs 数组的逐字顺序（= npm 安装参数顺序），分类是交错分布的，
        // 所以这里保持扁平：不要按分类重排。
        "@deepseek-ai/cordis@4.0.4",
        "@deepseek-ai/dsh-client-store@0.1.2-alpha.2",
        "@deepseek-ai/dsh-tools@0.2.0-rc.2",
        "@deepseek-ai/dsh-scope@0.2.0-rc.2",
        "@deepseek-ai/dsh-system-prompt@0.2.0-rc.2",
        "@deepseek-ai/dsh-subprocess@0.2.0-rc.2",
        "@deepseek-ai/dsh-subprocess-local@0.2.0-rc.2",
        "@deepseek-ai/dsh-llm@0.2.0-rc.2",
        "@deepseek-ai/dsh-sandbox@0.2.0-rc.2",
        "@deepseek-ai/dsh-http-proxy@0.2.0-rc.2",
        "@deepseek-ai/dsh-session@0.2.0-rc.2",
        "@deepseek-ai/dsh-session-projection@0.2.0-rc.2",
        "@deepseek-ai/dsh-agent@0.2.0-rc.2",
        "@deepseek-ai/dsh-invariants@0.2.0-rc.2",
        "@deepseek-ai/dsh-ptc-runtime@0.2.0-rc.2",
        "@deepseek-ai/dsh-sandbox-policy@0.2.0-rc.2",
        "@deepseek-ai/dsh-user-approval@0.2.0-rc.2",
        "@deepseek-ai/dsh-workspace@0.2.0-rc.2",
        "@deepseek-ai/dsh-util-values@0.2.0-rc.2",
        "@deepseek-ai/dsh-typert-protocol@0.2.0-rc.2",
        "@deepseek-ai/dsh-brand@0.2.0-rc.2",
        "@deepseek-ai/dsh-storage@0.2.0-rc.2",
        "@deepseek-ai/dsh-storage-domain@0.2.0-rc.2",
        "@deepseek-ai/dsh-session-persistence@0.2.0-rc.2",
        "@deepseek-ai/dsh-attachment@0.2.0-rc.2",
        "@deepseek-ai/dsh-timeout@0.2.0-rc.2",
        "@deepseek-ai/dsh-home-paths@0.2.0-rc.2",
        "@deepseek-ai/dsh-credentials@0.2.0-rc.2",
        "@deepseek-ai/dsh-client-connection@0.2.0-rc.2",
        "@deepseek-ai/dsh-client-ui-slots@0.2.0-rc.2",
        "@deepseek-ai/dsh-commands@0.2.0-rc.2",
        "@deepseek-ai/dsh-host-webserver@0.2.0-rc.2",
        "@deepseek-ai/dsh-agent-default-model@0.2.0-rc.2",
        "@deepseek-ai/dsh-client-ui-primitives@0.2.0-rc.2",
        "@deepseek-ai/dsh-util-code-language@0.2.0-rc.2",
        "@deepseek-ai/dsh-util-workspace-path@0.2.0-rc.2",
        "clsx@2.1.1",
        "diff@9.0.0",
        "anser@2.3.5",
        "katex@0.16.47",
        "shiki@4.3.1",
        "@shikijs/langs@4.3.1",
        "simple-icons@16.31.0",
        "mdast-util-from-markdown@2.0.3",
        "mdast-util-gfm@3.1.0",
        "mdast-util-math@3.0.0",
        "micromark-core-commonmark@2.0.3",
        "micromark-util-character@2.1.1",
        "micromark-util-classify-character@2.0.1",
        "micromark-util-symbol@2.0.1",
        "micromark-factory-space@2.0.1",
        "micromark-extension-math@3.1.0",
        "micromark-extension-gfm@3.0.0",
        "micromark-util-sanitize-uri@2.0.1",
        "react@18.3.1",
        "react-dom@18.3.1",
        "esbuild@0.25.12",
        "yaml@2.8.1",
    ];

    /// task-6 第 11.3 节实测「上游 primitives 在 npm 构建里外置」的 21 个包名（不带版本），
    /// 逐字抄自 .cache/plugin-verify/task6-classify.txt；少一个就说明 externalizedImports 过期。
    const EXTERNALIZED_REFERENCE: &[&str] = &[
        "@deepseek-ai/dsh-client-store",
        "@deepseek-ai/dsh-util-code-language",
        "@deepseek-ai/dsh-util-workspace-path",
        "clsx",
        "diff",
        "anser",
        "katex",
        "shiki",
        "@shikijs/langs",
        "simple-icons",
        "mdast-util-from-markdown",
        "mdast-util-gfm",
        "mdast-util-math",
        "micromark-core-commonmark",
        "micromark-util-character",
        "micromark-util-classify-character",
        "micromark-util-symbol",
        "micromark-factory-space",
        "micromark-extension-math",
        "micromark-extension-gfm",
        "micromark-util-sanitize-uri",
    ];

    fn manifest() -> serde_json::Value {
        serde_json::from_str(HOST_DEPENDENCIES_JSON)
            .expect("host-dependencies.json 必须是合法 JSON")
    }

    /// (分类 id, 理由)：分类只承载理由与归属校验，安装顺序由 `packages` 决定。
    fn categories(manifest: &serde_json::Value) -> Vec<(String, String)> {
        manifest["categories"]
            .as_array()
            .expect("categories 必须是数组")
            .iter()
            .map(|category| {
                let id = category["id"].as_str().expect("分类必须有字符串 id");
                let note = category["note"].as_str().expect("分类必须有字符串 note");
                (id.to_string(), note.to_string())
            })
            .collect()
    }

    /// (specifier, category)，逐字保持 JSON 里的书写顺序。
    fn entries(manifest: &serde_json::Value) -> Vec<(String, String)> {
        manifest["packages"]
            .as_array()
            .expect("packages 必须是数组")
            .iter()
            .map(|entry| {
                let specifier = entry["specifier"].as_str().expect("条目必须有 specifier");
                let category = entry["category"].as_str().expect("条目必须有 category");
                (specifier.to_string(), category.to_string())
            })
            .collect()
    }

    fn in_category(manifest: &serde_json::Value, id: &str) -> Vec<String> {
        entries(manifest)
            .into_iter()
            .filter(|(_, category)| category == id)
            .map(|(specifier, _)| specifier)
            .collect()
    }

    fn name_of(spec: &str) -> &str {
        spec.rsplit_once('@').expect("条目必须是 name@version").0
    }

    fn expanded(manifest: &serde_json::Value) -> Vec<String> {
        entries(manifest)
            .into_iter()
            .map(|(specifier, _)| specifier)
            .collect()
    }

    #[test]
    fn manifest_expands_to_the_frozen_install_order() {
        let manifest = manifest();
        let declared = categories(&manifest);
        let ids: Vec<&str> = declared.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["pluginPeers", "externalizedImports", "hostEntryPoints"]
        );
        let order = expanded(&manifest);
        assert_eq!(order.len(), 58, "宿主依赖清单必须正好 58 条");
        assert_eq!(order, FROZEN_ORDER, "展开顺序必须等于冻结的安装顺序");
        assert_eq!(
            [
                in_category(&manifest, "pluginPeers").len(),
                in_category(&manifest, "externalizedImports").len(),
                in_category(&manifest, "hostEntryPoints").len(),
            ],
            [23, 21, 14],
            "三类条数必须是 23 / 21 / 14（和 = 58，互斥且穷尽）"
        );
        for (id, note) in &declared {
            assert!(!note.trim().is_empty(), "{id} 分类必须写明理由");
            assert!(!in_category(&manifest, id).is_empty(), "{id} 分类不能为空");
        }
    }

    #[test]
    fn manifest_entries_are_unique_name_at_version() {
        let order = expanded(&manifest());
        let mut seen = std::collections::HashSet::new();
        for entry in &order {
            let (name, version) = entry.rsplit_once('@').expect("条目必须是 name@version");
            assert!(!name.is_empty(), "条目 {entry} 缺少包名");
            assert!(!version.is_empty(), "条目 {entry} 缺少版本");
            assert!(!version.contains('@'), "条目 {entry} 的版本不能含 @");
            if name.starts_with('@') {
                assert!(name[1..].contains('/'), "作用域包 {entry} 必须带包名");
            } else {
                assert!(!name.contains('/'), "非作用域包 {entry} 不能带 /");
            }
            assert!(seen.insert(entry.clone()), "条目 {entry} 重复");
        }
        assert_eq!(seen.len(), order.len(), "类间/类内都不得重复");
    }

    #[test]
    fn externalized_imports_cover_the_task6_reference() {
        let externalized = in_category(&manifest(), "externalizedImports");
        assert!(!externalized.is_empty(), "必须有 externalizedImports 分类");
        let names: Vec<&str> = externalized.iter().map(|entry| name_of(entry)).collect();
        assert_eq!(
            names.len(),
            EXTERNALIZED_REFERENCE.len(),
            "外置 import 必须正好 21 条"
        );
        for reference in EXTERNALIZED_REFERENCE {
            assert!(
                names.contains(reference),
                "外置 import 缺少 {reference}（task-6 实测它被 primitives 外置）"
            );
        }
        for entry in &externalized {
            assert!(
                EXTERNALIZED_REFERENCE.contains(&name_of(entry)),
                "清单里的 {entry} 不在 task-6 实测集合里"
            );
        }
    }

    #[test]
    fn host_dependencies_expands_every_entry_once() {
        let order = expanded(&manifest());
        let resolved = host_dependencies().expect("清单必须能解析");
        assert_eq!(
            resolved, order,
            "host_dependencies() 必须按书写顺序展开全部条目"
        );
        let again = host_dependencies().expect("清单必须能解析");
        assert_eq!(resolved, again, "缓存后的结果必须与首次一致");
    }

    #[test]
    fn manifest_parser_rejects_malformed_input() {
        let peers = || serde_json::json!([{ "id": "pluginPeers", "note": "n" }]);
        let good = serde_json::json!({
            "categories": peers(),
            "packages": [{ "specifier": "a@1.0.0", "category": "pluginPeers" }],
        });
        assert_eq!(
            parse_host_dependencies(&good.to_string()).unwrap(),
            vec!["a@1.0.0"]
        );
        assert!(parse_host_dependencies("not json").is_err());
        assert!(parse_host_dependencies("{}").is_err());
        assert!(parse_host_dependencies("[]").is_err());
        // categories 缺失 / 为空
        assert!(
            parse_host_dependencies(&serde_json::json!({ "packages": [] }).to_string()).is_err()
        );
        assert!(parse_host_dependencies(
            &serde_json::json!({ "categories": [], "packages": [] }).to_string()
        )
        .is_err());
        // categories 里的分类缺 id
        assert!(parse_host_dependencies(
            &serde_json::json!({ "categories": [{ "note": "n" }], "packages": [] }).to_string()
        )
        .is_err());
        // packages 缺失、条目缺 specifier、specifier 不是字符串
        assert!(
            parse_host_dependencies(&serde_json::json!({ "categories": peers() }).to_string())
                .is_err()
        );
        assert!(parse_host_dependencies(
            &serde_json::json!({ "categories": peers(), "packages": [{ "category": "pluginPeers" }] })
                .to_string()
        )
        .is_err());
        assert!(parse_host_dependencies(
            &serde_json::json!({
                "categories": peers(),
                "packages": [{ "specifier": 1, "category": "pluginPeers" }],
            })
            .to_string()
        )
        .is_err());
        // 条目引用了 categories 里没有的分类
        assert!(parse_host_dependencies(
            &serde_json::json!({
                "categories": peers(),
                "packages": [{ "specifier": "a@1.0.0", "category": "nope" }],
            })
            .to_string()
        )
        .is_err());
    }

    /// 摘要算法冻结：host-revision 必须 = sha256(7 个脚本 + 逐条 specifier + 分隔符)，
    /// **不包含清单 JSON 原文**。若有人把它改回「脚本 + 原文」，note 散文一改就会让所有
    /// 已装插件重写脚本、本机插件重建图，这条断言会立刻变红。
    #[test]
    fn host_revision_tracks_scripts_and_specifiers_only() {
        let mut expected = Sha256::new();
        for script in HOST_SCRIPTS {
            expected.update(script.as_bytes());
        }
        for specifier in host_dependencies().expect("清单可解析") {
            expected.update(specifier.as_bytes());
            expected.update(b"\n");
        }
        assert_eq!(host_revision(), format!("{:x}", expected.finalize()));

        // 与「脚本 + 清单原文」的旧口径必须不同，即文档改动不再影响摘要。
        let mut raw = Sha256::new();
        for script in HOST_SCRIPTS {
            raw.update(script.as_bytes());
        }
        raw.update(HOST_DEPENDENCIES_JSON.as_bytes());
        assert_ne!(host_revision(), format!("{:x}", raw.finalize()));

        // 同一份清单两次调用必须一致（local.rs 与 prepare_runtime 要能对上）。
        assert_eq!(host_revision(), host_revision());
    }
}

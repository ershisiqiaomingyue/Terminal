//! Import installed package files and their physical dependency graph. npm is
//! used only for missing shared host dependencies, never for the local plugin.
use super::*;

pub(crate) fn upgrade(app: &AppHandle, plugin: &Plugin) -> Result<Plugin, String> {
    let path = directory(app, &plugin.id)?
        .join("versions")
        .join(&plugin.install_directory);
    if !path.join("local-plugin.json").is_file()
        || fs::read_to_string(path.join("local-graph-revision"))
            .ok()
            .as_deref()
            == Some(&host_revision())
    {
        return Ok(plugin.clone());
    }
    // Rebuild from our own copy: the original profile may already be gone.
    // A new version keeps the old graph and the separate data directory intact.
    let source = path.join("node_modules").join(&plugin.name);
    let mut progress = progress::InstallProgress::new(app, "");
    let replacement = super::install(
        app,
        &source.to_string_lossy(),
        &plugin.required_environment_tools,
        &mut progress,
    )?;
    let mut updated = plugin.clone();
    updated.install_directory = replacement.install_directory;
    save(app, &updated)?;
    Ok(updated)
}

fn candidates(app: &AppHandle, cache: &Path) -> Result<Vec<PathBuf>, String> {
    let mut result = vec![cache.to_path_buf()];
    for plugin in installed(app)? {
        if !plugin.install_directory.is_empty()
            && plugin.install_directory.bytes().all(|c| c.is_ascii_digit())
        {
            result.push(
                directory(app, &plugin.id)?
                    .join("versions")
                    .join(plugin.install_directory),
            );
        }
    }
    let profile_root = std::env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".dsh")));
    if let Some(root) = profile_root {
        if let Ok(profiles) = fs::read_dir(root.join("profiles")) {
            result.extend(profiles.flatten().map(|entry| entry.path()));
        }
    }
    Ok(result)
}

pub(super) fn install(
    app: &AppHandle,
    source: &str,
    path: &Path,
    pipeline: &Pipeline,
    progress: &mut progress::InstallProgress<'_>,
) -> Result<(), String> {
    let cache = root(app)?.join("runtime");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let script = path.join("local-packages.mjs");
    let config = path.join("local-import.json");
    let report = path.join("local-import-result.json");
    fs::write(&script, include_str!("../local-packages.mjs")).map_err(|e| e.to_string())?;
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "source": source, "directory": path, "hostDependencies": host_dependencies()?,
            "candidates": candidates(app, &cache)?,
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let invoke = |mode: &str| -> Result<(), String> {
        let mut command = pipeline.launch_command(path, &script)?;
        command.arg(&config).arg(mode).arg(&report);
        run(command, path, Duration::from_secs(300), None)
    };
    invoke("check")?;
    let report_value: serde_json::Value =
        serde_json::from_slice(&fs::read(&report).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let missing = report_value["missing"]
        .as_array()
        .ok_or("本地宿主依赖检查结果无效")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or("本地宿主依赖检查结果无效")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !missing.is_empty() {
        // Cache shared host services separately. Installing into the plugin's
        // copy would let npm prune/re-resolve its already-installed packages.
        if !cache.join("package.json").is_file() {
            fs::write(
                cache.join("package.json"),
                "{\"private\":true,\"type\":\"module\"}",
            )
            .map_err(|e| e.to_string())?;
        }
        progress.phase("downloading-host");
        run(
            pipeline.install_command(
                &cache,
                &missing,
                InstallOptions {
                    local_links: true,
                    observe: true,
                },
            )?,
            &cache,
            Duration::from_secs(300),
            Some(progress),
        )?;
    }
    progress.phase("copying");
    invoke("copy")?;
    fs::write(path.join("local-graph-revision"), host_revision()).map_err(|e| e.to_string())
}

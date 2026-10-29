use super::*;

pub(super) fn collect_jadx_sources(
    root: &Path,
    path: &Path,
    files: &mut Vec<String>,
) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|error| format!("无法读取反编译目录：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取反编译目录：{error}"))?
            .path();
        if path.is_dir() {
            collect_jadx_sources(root, &path, files)?;
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "java" | "kt"))
        {
            if files.len() < 20_000 {
                files.push(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    Ok(())
}

pub(super) fn editable_workspace_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "xml" | "json" | "txt" | "smali" | "properties" | "yml" | "yaml" | "cfg"
            )
        })
}

pub(super) fn manifest_package_name(manifest: &str) -> Option<&str> {
    let manifest_tag = manifest.split_once("<manifest")?.1.split_once('>')?.0;
    let package_start = manifest_tag.find("package=")? + "package=".len();
    let quote = manifest_tag.as_bytes().get(package_start).copied()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let value = &manifest_tag[package_start + 1..];
    let package_name = value.split_once(quote as char)?.0;
    (!package_name.is_empty()
        && package_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_')))
    .then_some(package_name)
}

/// Apktool 3 treats an APK with resource package ID 0 and an empty resource
/// package name as a shared library, then invokes aapt2 with an invalid empty
/// `--rename-resources-package` argument. Keep this repair in the isolated
/// workspace; the original APK remains untouched.
pub(super) fn repair_apktool_resource_package(
    decoded: &Path,
    manifest: &str,
) -> Result<bool, String> {
    let package_name = match manifest_package_name(manifest) {
        Some(package_name) => package_name,
        None => return Ok(false),
    };
    let yaml_path = decoded.join("apktool.yml");
    let yaml = fs::read_to_string(&yaml_path)
        .map_err(|error| format!("无法读取 Apktool 配置：{error}"))?;
    if !yaml.contains("packageId: 0") {
        return Ok(false);
    }
    let mut repaired = false;
    let rewritten = yaml
        .lines()
        .map(|line| {
            if !repaired && line.trim() == "packageName:" {
                repaired = true;
                let indent = &line[..line.len() - line.trim_start().len()];
                format!("{indent}packageName: {package_name}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if repaired {
        fs::write(yaml_path, rewritten)
            .map_err(|error| format!("无法修复 Apktool 配置：{error}"))?;
    }
    Ok(repaired)
}

pub(super) fn workspace_file_path(
    app: &AppHandle,
    session_id: &str,
    relative_path: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let root = archive_session_root(app, session_id)?;
    let decoded = root.join("decoded");
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("编辑文件路径无效。".to_string());
    }
    let candidate = fs::canonicalize(decoded.join(relative))
        .map_err(|error| format!("无法读取编辑文件：{error}"))?;
    if !candidate.starts_with(&decoded)
        || !candidate.is_file()
        || !editable_workspace_file(&candidate)
    {
        return Err("该文件不支持编辑。".to_string());
    }
    Ok((root, candidate))
}
pub(super) fn collect_editable_workspace_files(
    root: &Path,
    path: &Path,
    files: &mut Vec<WorkspaceFile>,
) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|error| format!("无法读取工作区：{error}"))? {
        let path = entry
            .map_err(|error| format!("无法读取工作区：{error}"))?
            .path();
        if path.is_dir() {
            collect_editable_workspace_files(root, &path, files)?;
        } else if editable_workspace_file(&path) && files.len() < 20_000 {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let baseline = root
                .parent()
                .unwrap_or(root)
                .join("baseline")
                .join(&relative);
            files.push(WorkspaceFile {
                path: relative,
                size: fs::metadata(&path)
                    .map(|metadata| metadata.len())
                    .unwrap_or_default(),
                changed: baseline.is_file() && fs::read(&baseline).ok() != fs::read(&path).ok(),
            });
        }
    }
    Ok(())
}

pub(super) fn workspace_change_category(path: &str) -> &'static str {
    if path == "AndroidManifest.xml" {
        "Manifest"
    } else if path.starts_with("smali") {
        "smali"
    } else if path.starts_with("res/") {
        "资源"
    } else if path.starts_with("assets/") {
        "assets"
    } else {
        "其他"
    }
}

/// Count the changed middle section after trimming the identical prefix and
/// suffix. This is bounded, predictable, and gives a useful summary without
/// holding a full LCS matrix for large decoded APK workspaces.
pub(super) fn changed_line_counts(original: &str, current: &str) -> (usize, usize) {
    let before = original.lines().collect::<Vec<_>>();
    let after = current.lines().collect::<Vec<_>>();
    // Most resource and smali edits are small. For these, calculate the exact
    // LCS with one row of memory. Very large files use the bounded fallback
    // below so a summary request cannot make the desktop UI stall.
    if before.len().saturating_mul(after.len()) <= 250_000 {
        let mut row = vec![0usize; after.len() + 1];
        for original_line in &before {
            let mut diagonal = 0;
            for (index, current_line) in after.iter().enumerate() {
                let above = row[index + 1];
                row[index + 1] = if original_line == current_line {
                    diagonal + 1
                } else {
                    row[index + 1].max(row[index])
                };
                diagonal = above;
            }
        }
        let common = row[after.len()];
        return (after.len() - common, before.len() - common);
    }
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before.len().saturating_sub(prefix)
        && suffix < after.len().saturating_sub(prefix)
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    (
        after.len() - prefix - suffix,
        before.len() - prefix - suffix,
    )
}

pub(super) fn xml_attribute_values(content: &str) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    for tag in content.split('<').skip(1) {
        let Some(tag) = tag.split('>').next() else {
            continue;
        };
        let mut parts = tag.trim().trim_start_matches('/').split_whitespace();
        let Some(element) = parts.next() else {
            continue;
        };
        if element.starts_with('!') || element.starts_with('?') {
            continue;
        }
        for part in parts {
            let Some((name, value)) = part.trim_end_matches('/').split_once('=') else {
                continue;
            };
            let value = value.trim_matches(|character| character == '\'' || character == '"');
            if !name.is_empty() && !value.is_empty() {
                attributes.insert(format!("{element}@{name}"), value.to_string());
            }
        }
    }
    attributes
}

pub(super) fn xml_resource_values(content: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for fragment in content.split('<').skip(1) {
        let Some((tag, body)) = fragment.split_once('>') else {
            continue;
        };
        let tag = tag.trim().trim_start_matches('/');
        let Some(element) = tag.split_whitespace().next() else {
            continue;
        };
        if element.starts_with('!') || element.starts_with('?') || tag.starts_with('/') {
            continue;
        }
        let name = tag.split_whitespace().find_map(|part| {
            let (key, value) = part.trim_end_matches('/').split_once('=')?;
            (key == "name").then(|| {
                value
                    .trim_matches(|character| character == '\'' || character == '"')
                    .to_string()
            })
        });
        let value = body.split('<').next().unwrap_or_default().trim();
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            values.insert(format!("{element}#{name}"), value.to_string());
        }
    }
    values
}

pub(super) fn smali_methods(content: &str) -> BTreeMap<String, ()> {
    content
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix(".method ")
                .map(|method| (method.to_string(), ()))
        })
        .collect()
}

pub(super) fn changed_map_keys<T: PartialEq>(
    before: &BTreeMap<String, T>,
    after: &BTreeMap<String, T>,
) -> (usize, usize, Vec<String>) {
    let added = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let removed = before
        .iter()
        .filter(|(key, value)| after.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let mut samples = added
        .iter()
        .chain(removed.iter())
        .take(3)
        .cloned()
        .collect::<Vec<_>>();
    samples.sort();
    samples.dedup();
    (added.len(), removed.len(), samples)
}

pub(super) fn structured_map_changes(
    kind: &str,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    limit: usize,
) -> Vec<WorkspaceStructuredChange> {
    before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .take(limit)
        .map(|key| WorkspaceStructuredChange {
            kind: kind.to_string(),
            key: key.to_string(),
            before: before.get(key).cloned(),
            after: after.get(key).cloned(),
        })
        .collect()
}

pub(super) fn workspace_structured_changes(
    path: &str,
    original: &str,
    current: &str,
) -> Vec<WorkspaceStructuredChange> {
    const MAX_CHANGES: usize = 16;
    if path.ends_with(".smali") {
        let before = smali_methods(original)
            .into_keys()
            .map(|key| (key, String::new()))
            .collect();
        let after = smali_methods(current)
            .into_keys()
            .map(|key| (key, String::new()))
            .collect();
        return structured_map_changes("smali 方法", &before, &after, MAX_CHANGES);
    }
    if path.ends_with(".xml") {
        let mut changes = structured_map_changes(
            "XML 属性",
            &xml_attribute_values(original),
            &xml_attribute_values(current),
            MAX_CHANGES,
        );
        if changes.len() < MAX_CHANGES && path.starts_with("res/") {
            changes.extend(structured_map_changes(
                "资源键",
                &xml_resource_values(original),
                &xml_resource_values(current),
                MAX_CHANGES - changes.len(),
            ));
        }
        return changes;
    }
    Vec::new()
}

pub(super) fn workspace_change_detail(path: &str, original: &str, current: &str) -> String {
    if path.ends_with(".smali") {
        let (added, removed, samples) =
            changed_map_keys(&smali_methods(original), &smali_methods(current));
        return if added == 0 && removed == 0 {
            "smali 指令或字段已改动".to_string()
        } else {
            format!(
                "方法 +{added} / -{removed}{}",
                if samples.is_empty() {
                    String::new()
                } else {
                    format!("：{}", samples.join("、"))
                }
            )
        };
    }
    if path.ends_with(".xml") {
        let (added, removed, samples) = changed_map_keys(
            &xml_attribute_values(original),
            &xml_attribute_values(current),
        );
        return if added == 0 && removed == 0 {
            "XML 节点或文本已改动".to_string()
        } else {
            format!(
                "属性 +{added} / -{removed}{}",
                if samples.is_empty() {
                    String::new()
                } else {
                    format!("：{}", samples.join("、"))
                }
            )
        };
    }
    "文本内容已改动".to_string()
}

pub(super) fn archive_session_root(app: &AppHandle, session_id: &str) -> Result<PathBuf, String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("无效的编辑工作区。".to_string());
    }
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("archive-workspaces")
        .join(session_id);
    root.is_dir()
        .then_some(root)
        .ok_or_else(|| "编辑工作区不存在或已被清理。".to_string())
}

pub(super) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub(super) fn archive_history_store(app: &AppHandle) -> Result<Connection, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("archive-history.sqlite3");
    let connection =
        Connection::open(path).map_err(|error| format!("无法打开 APK 历史：{error}"))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("无法配置 APK 历史数据库：{error}"))?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS archive_history (id TEXT PRIMARY KEY, archive_path TEXT NOT NULL, file_name TEXT NOT NULL, sha256 TEXT NOT NULL, opened_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, workspace_path TEXT, apktool_version TEXT, manifest_changed INTEGER NOT NULL DEFAULT 0, output_path TEXT, verification TEXT, installed_serial TEXT, install_result TEXT)").map_err(|error| format!("无法初始化 APK 历史：{error}"))?;
    Ok(connection)
}
pub(super) fn upsert_archive_history(
    app: &AppHandle,
    record: &ArchiveHistoryRecord,
) -> Result<(), String> {
    archive_history_store(app)?.execute("INSERT INTO archive_history (id,archive_path,file_name,sha256,opened_at,updated_at,workspace_path,apktool_version,manifest_changed,output_path,verification,installed_serial,install_result) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT(id) DO UPDATE SET archive_path=excluded.archive_path,file_name=excluded.file_name,sha256=excluded.sha256,opened_at=excluded.opened_at,updated_at=excluded.updated_at,workspace_path=excluded.workspace_path,apktool_version=excluded.apktool_version,manifest_changed=excluded.manifest_changed,output_path=excluded.output_path,verification=excluded.verification,installed_serial=excluded.installed_serial,install_result=excluded.install_result", params![record.id,record.archive_path,record.file_name,record.sha256,record.opened_at,record.updated_at,record.workspace_path,record.apktool_version,record.manifest_changed as i32,record.output_path,record.verification,record.installed_serial,record.install_result]).map_err(|error| format!("无法保存 APK 历史：{error}"))?;
    Ok(())
}

pub(super) fn record_archive_opened(
    app: &AppHandle,
    inspection: &ArchiveInspection,
) -> Result<(), String> {
    let timestamp = now_millis();
    upsert_archive_history(
        app,
        &ArchiveHistoryRecord {
            id: inspection.sha256.clone(),
            archive_path: inspection.path.clone(),
            file_name: inspection.file_name.clone(),
            sha256: inspection.sha256.clone(),
            opened_at: timestamp,
            updated_at: timestamp,
            workspace_path: None,
            apktool_version: None,
            manifest_changed: false,
            output_path: None,
            verification: None,
            installed_serial: None,
            install_result: None,
        },
    )
}

pub(super) fn decoded_manifest_preview(source: &Path) -> Option<String> {
    let aapt2 = find_aapt2()?;
    let mut command = Command::new(aapt2);
    command
        .args(["dump", "xmltree", "--file", "AndroidManifest.xml"])
        .arg(source);
    let output =
        command_output_with_timeout_limit(command, Duration::from_secs(30), MAX_AAPT_OUTPUT_BYTES)
            .ok()?;
    if !output.status.success() {
        return None;
    }
    axml_tree_to_xml(&String::from_utf8_lossy(&output.stdout))
}

pub(super) fn find_java_decompiler() -> Option<(PathBuf, PathBuf)> {
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let studio = program_files.join("Android").join("Android Studio");
    let java = if cfg!(target_os = "windows") {
        studio.join("jbr").join("bin").join("java.exe")
    } else {
        studio.join("jbr").join("bin").join("java")
    };
    let decompiler = studio
        .join("plugins")
        .join("java-decompiler")
        .join("lib")
        .join("java-decompiler.jar");
    (java.is_file() && decompiler.is_file()).then_some((java, decompiler))
}

pub(super) fn find_decompiled_java(directory: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(directory).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(java) = find_decompiled_java(&path) {
                return Some(java);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("java"))
        {
            return Some(path);
        }
    }
    None
}

pub(super) fn decompile_class_bytes(data: &[u8], entry_path: &str) -> Option<String> {
    let (java, decompiler) = find_java_decompiler()?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let work = std::env::temp_dir().join(format!(
        "android-tool-class-{}-{timestamp}",
        std::process::id()
    ));
    let output_dir = work.join("out");
    let result = (|| -> Option<String> {
        fs::create_dir_all(&output_dir).ok()?;
        let file_name = Path::new(entry_path).file_name()?;
        let class_file = work.join(file_name);
        let mut output = File::create(&class_file).ok()?;
        output.write_all(data).ok()?;
        output.flush().ok()?;
        let mut process = Command::new(java);
        process
            .arg("-cp")
            .arg(decompiler)
            .arg("org.jetbrains.java.decompiler.main.decompiler.ConsoleDecompiler")
            .arg(&class_file)
            .arg(&output_dir);
        let command = command_output_with_timeout_limit(
            process,
            Duration::from_secs(60),
            MAX_AAPT_OUTPUT_BYTES,
        )
        .ok()?;
        if !command.status.success() {
            return None;
        }
        let java_file = find_decompiled_java(&output_dir)?;
        if fs::metadata(&java_file).ok()?.len() > MAX_CLASS_DECOMPILED_BYTES {
            return None;
        }
        fs::read_to_string(java_file).ok()
    })();
    let _ = fs::remove_dir_all(work);
    result.filter(|content| !content.trim().is_empty())
}

pub(super) fn decompile_class_entry(source: &Path, entry_path: &str) -> Option<String> {
    let archive_file = File::open(source).ok()?;
    let mut archive = ZipArchive::new(archive_file).ok()?;
    let mut entry = archive.by_name(entry_path).ok()?;
    if entry.size() > MAX_CLASS_BYTES {
        return None;
    }
    let mut data = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut data).ok()?;
    decompile_class_bytes(&data, entry_path)
}

pub(super) fn aar_embedded_entry(source: &Path, entry_path: &str) -> Option<(u64, Vec<u8>)> {
    let inner_path = entry_path.strip_prefix("classes.jar/")?;
    let archive_file = File::open(source).ok()?;
    let mut archive = ZipArchive::new(archive_file).ok()?;
    let mut classes_jar = archive.by_name("classes.jar").ok()?;
    let mut jar_data = Vec::with_capacity(classes_jar.size() as usize);
    classes_jar.read_to_end(&mut jar_data).ok()?;
    let mut nested = ZipArchive::new(Cursor::new(jar_data)).ok()?;
    let mut entry = nested.by_name(inner_path).ok()?;
    if entry.is_dir() {
        return None;
    }
    let size = entry.size();
    let mut data = Vec::with_capacity((size as usize).min(MAX_PREVIEW_BYTES + 1));
    entry
        .by_ref()
        .take((MAX_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .ok()?;
    Some((size, data))
}

pub(super) fn preview_from_bytes(
    path: String,
    size: u64,
    mut data: Vec<u8>,
) -> ArchiveEntryPreview {
    let truncated = data.len() > MAX_PREVIEW_BYTES || size > MAX_PREVIEW_BYTES as u64;
    if data.len() > MAX_PREVIEW_BYTES {
        data.truncate(MAX_PREVIEW_BYTES);
    }
    let binary = data
        .iter()
        .any(|byte| *byte == 0 || (*byte < 0x09) || (*byte > 0x0d && *byte < 0x20));
    let (kind, content) = if binary {
        let lines = data
            .chunks(16)
            .take(4096)
            .enumerate()
            .map(|(index, row)| {
                let hex = row
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{:#08x}  {hex}", index * 16)
            })
            .collect::<Vec<_>>()
            .join("\n");
        ("binary".to_string(), lines)
    } else {
        (
            "text".to_string(),
            String::from_utf8_lossy(&data).to_string(),
        )
    };
    ArchiveEntryPreview {
        path,
        size,
        kind,
        content,
        truncated,
    }
}

#[derive(Default)]
pub(super) struct XmlNode {
    pub(super) name: String,
    pub(super) attributes: Vec<(String, String)>,
    pub(super) children: Vec<XmlNode>,
}

pub(super) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(super) fn axml_attribute(line: &str) -> Option<(String, String)> {
    let attribute = line.trim_start().strip_prefix("A: ")?;
    let (key, value) = attribute.split_once('=')?;
    let key = key.split('(').next()?.trim();
    let name = if let Some(attribute_name) =
        key.strip_prefix("http://schemas.android.com/apk/res/android:")
    {
        format!("android:{attribute_name}")
    } else {
        key.rsplit_once(':')
            .map(|(_, name)| name)
            .unwrap_or(key)
            .to_string()
    };
    let raw_marker = "(Raw: \"";
    let value = value
        .find(raw_marker)
        .and_then(|start| {
            let raw = &value[start + raw_marker.len()..];
            raw.rfind("\")").map(|end| raw[..end].to_string())
        })
        .unwrap_or_else(|| value.trim().to_string());
    Some((name, value))
}

pub(super) fn render_xml(node: &XmlNode, depth: usize, output: &mut String) {
    let indent = "  ".repeat(depth);
    output.push_str(&indent);
    output.push('<');
    output.push_str(&node.name);
    for (name, value) in &node.attributes {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        output.push_str(&xml_escape(value));
        output.push('"');
    }
    if node.children.is_empty() {
        output.push_str(" />\n");
        return;
    }
    output.push_str(">\n");
    for child in &node.children {
        render_xml(child, depth + 1, output);
    }
    output.push_str(&indent);
    output.push_str("</");
    output.push_str(&node.name);
    output.push_str(">\n");
}

pub(super) fn axml_tree_to_xml(tree: &str) -> Option<String> {
    let mut namespaces = Vec::new();
    let mut root: Option<XmlNode> = None;
    let mut stack: Vec<(usize, XmlNode)> = Vec::new();

    for line in tree.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if let Some(namespace) = trimmed.strip_prefix("N: ") {
            if let Some((prefix, uri)) = namespace.split_once('=') {
                namespaces.push((prefix.trim().to_string(), uri.trim().to_string()));
            }
            continue;
        }
        if let Some(element) = trimmed.strip_prefix("E: ") {
            let name = element.split_whitespace().next()?.to_string();
            while stack.last().is_some_and(|(level, _)| *level >= indent) {
                let (_, child) = stack.pop()?;
                if let Some((_, parent)) = stack.last_mut() {
                    parent.children.push(child);
                } else {
                    root = Some(child);
                }
            }
            stack.push((
                indent,
                XmlNode {
                    name,
                    ..Default::default()
                },
            ));
        } else if trimmed.starts_with("A: ") {
            if let (Some((_, node)), Some(attribute)) = (stack.last_mut(), axml_attribute(trimmed))
            {
                node.attributes.push(attribute);
            }
        }
    }
    while let Some((_, child)) = stack.pop() {
        if let Some((_, parent)) = stack.last_mut() {
            parent.children.push(child);
        } else {
            root = Some(child);
        }
    }
    let mut root = root?;
    for (prefix, uri) in namespaces {
        let name = if prefix.is_empty() {
            "xmlns".to_string()
        } else {
            format!("xmlns:{prefix}")
        };
        root.attributes.insert(0, (name, uri));
    }
    let mut output = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    render_xml(&root, 0, &mut output);
    Some(output)
}

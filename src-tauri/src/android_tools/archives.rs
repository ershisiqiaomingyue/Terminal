use super::*;

impl AndroidPackageKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Apk => "APK",
            Self::Aar => "AAR",
            Self::Jar => "JAR",
            Self::Aab => "AAB",
            Self::Apks => "APKS",
            Self::Zip => "ZIP",
            Self::So => "ELF SO",
        }
    }
}

pub(super) fn android_package_file_name_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\.(apk|aar|jar|aab|apks|zip|so)(?:\d+|\.\d+|\s*\(\d+\))?$")
            .expect("Android package file name pattern is valid")
    })
}

pub(super) fn android_package_kind(path: &Path) -> Option<AndroidPackageKind> {
    let file_name = path.file_name()?.to_str()?.trim();
    match android_package_file_name_pattern()
        .captures(file_name)?
        .get(1)?
        .as_str()
        .to_ascii_lowercase()
        .as_str()
    {
        "apk" => Some(AndroidPackageKind::Apk),
        "aar" => Some(AndroidPackageKind::Aar),
        "jar" => Some(AndroidPackageKind::Jar),
        "aab" => Some(AndroidPackageKind::Aab),
        "apks" => Some(AndroidPackageKind::Apks),
        "zip" => Some(AndroidPackageKind::Zip),
        "so" => Some(AndroidPackageKind::So),
        _ => None,
    }
}

pub(super) fn archive_path(path: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path.trim()).map_err(|error| format!("无法读取文件：{error}"))?;
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取文件信息：{error}"))?;
    if !metadata.is_file() {
        return Err("请选择一个 Android 包或原生库文件。".to_string());
    }
    if android_package_kind(&path).is_none() {
        return Err("仅支持 APK、AAR、JAR、AAB、APKS、ZIP 或 SO 文件。".to_string());
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err("文件超过 2 GB 的首版处理限制。".to_string());
    }
    Ok(path)
}

pub(super) fn inspect_native_shared_object(
    app: &AppHandle,
    path: &Path,
    task: Option<(&str, &AtomicBool)>,
) -> Result<ArchiveInspection, String> {
    if let Some((task_id, cancellation)) = task {
        if cancellation.load(Ordering::Relaxed) {
            return Err("原生库检查已取消。".to_string());
        }
        android_tasks::update_task(app, task_id, 20, "正在读取 ELF 文件头...")?;
    }
    let metadata = fs::metadata(path).map_err(|error| format!("无法读取原生库信息：{error}"))?;
    let mut header = [0u8; 20];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("无法读取 ELF 文件头：{error}"))?;
    if &header[..4] != b"\x7fELF" {
        return Err("SO 文件不是有效的 ELF 原生库。".to_string());
    }
    let class = match header[4] {
        1 => "32 位",
        2 => "64 位",
        _ => "未知位数",
    };
    let little_endian = header[5] == 1;
    let byte_order = if little_endian {
        "Little Endian"
    } else if header[5] == 2 {
        "Big Endian"
    } else {
        "未知字节序"
    };
    let machine = if little_endian {
        u16::from_le_bytes([header[18], header[19]])
    } else {
        u16::from_be_bytes([header[18], header[19]])
    };
    let architecture = match machine {
        3 => "x86",
        40 => "armeabi-v7a",
        62 => "x86_64",
        183 => "arm64-v8a",
        243 => "riscv64",
        _ => "未知架构",
    };
    let sha256 = sha256_with_task(
        path,
        task.map(|(task_id, cancellation)| (app, task_id, cancellation, 55, 40)),
    )?;
    let inspection = ArchiveInspection {
        path: path.to_string_lossy().to_string(),
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("library.so")
            .to_string(),
        format: format!("ELF SO · {architecture} · {class} · {byte_order}"),
        size: metadata.len(),
        sha256,
        entries: Vec::new(),
        entry_count: 0,
        compressed_size: metadata.len(),
        uncompressed_size: metadata.len(),
    };
    record_archive_opened(app, &inspection)?;
    Ok(inspection)
}

pub(super) fn should_index_embedded_aar_jar(is_aar: bool, entry_name: &str, size: u64) -> bool {
    is_aar && entry_name == "classes.jar" && size <= MAX_AAR_EMBEDDED_JAR_INDEX_BYTES
}

pub(super) fn sha256(path: &Path) -> Result<String, String> {
    sha256_with_task(path, None)
}

pub(super) fn sha256_with_task(
    path: &Path,
    task: Option<(&AppHandle, &str, &AtomicBool, u8, u8)>,
) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("无法打开文件：{error}"))?;
    let total = fs::metadata(path)
        .map_err(|error| format!("无法读取文件信息：{error}"))?
        .len()
        .max(1);
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    let mut processed = 0u64;
    loop {
        if let Some((app, task_id, cancellation, start, span)) = task {
            if cancellation.load(Ordering::Relaxed) {
                return Err("压缩包检查已取消。".to_string());
            }
            let progress =
                start.saturating_add(((processed.saturating_mul(span as u64)) / total) as u8);
            android_tasks::update_task(app, task_id, progress, "正在计算压缩包 SHA-256...")?;
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法读取文件：{error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        processed = processed.saturating_add(read as u64);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn archive_entry_digests(
    source: &Path,
    task: Option<(&AppHandle, &str, &AtomicBool, u8, u8)>,
) -> Result<BTreeMap<String, String>, String> {
    let file = File::open(source).map_err(|error| format!("无法打开压缩包：{error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("不是可读取的 ZIP/APK/AAR/JAR 文件：{error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("压缩包条目过多，超过对比限制。".to_string());
    }
    let mut result = BTreeMap::new();
    let mut buffer = [0u8; 128 * 1024];
    for index in 0..archive.len() {
        if let Some((app, task_id, cancellation, start, span)) = task {
            if cancellation.load(Ordering::Relaxed) {
                return Err("深度对比已取消。".to_string());
            }
            if index % 100 == 0 {
                let progress =
                    start.saturating_add(((index * span as usize) / archive.len().max(1)) as u8);
                android_tasks::update_task(
                    app,
                    task_id,
                    progress,
                    &format!(
                        "正在校验 {} 的 {index}/{} 个条目...",
                        source
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("压缩包"),
                        archive.len()
                    ),
                )?;
            }
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取压缩包条目：{error}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let mut digest = Sha256::new();
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| format!("无法读取压缩包内容：{error}"))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        result.insert(name, format!("{:x}", digest.finalize()));
    }
    Ok(result)
}

#[tauri::command]
pub fn compare_android_archives(
    request: ArchiveCompareRequest,
) -> Result<ArchiveCompareResult, String> {
    compare_android_archives_inner(&request, None)
}

pub(super) fn compare_android_archives_inner(
    request: &ArchiveCompareRequest,
    task: Option<(&AppHandle, &str, &AtomicBool)>,
) -> Result<ArchiveCompareResult, String> {
    let left = archive_path(&request.left_path)?;
    let right = archive_path(&request.right_path)?;
    if let Some((app, task_id, cancellation)) = task {
        android_tasks::update_task(app, task_id, 2, "正在读取左侧压缩包...")?;
        if cancellation.load(Ordering::Relaxed) {
            return Err("深度对比已取消。".to_string());
        }
    }
    let left_entries = archive_entry_digests(
        &left,
        task.map(|(app, id, cancellation)| (app, id, cancellation, 5, 43)),
    )?;
    let right_entries = archive_entry_digests(
        &right,
        task.map(|(app, id, cancellation)| (app, id, cancellation, 48, 43)),
    )?;
    if let Some((app, task_id, cancellation)) = task {
        if cancellation.load(Ordering::Relaxed) {
            return Err("深度对比已取消。".to_string());
        }
        android_tasks::update_task(app, task_id, 93, "正在汇总压缩包差异...")?;
    }
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged_count = 0usize;
    for (path, digest) in &right_entries {
        match left_entries.get(path) {
            None => added.push(path.clone()),
            Some(left_digest) if left_digest != digest => changed.push(path.clone()),
            Some(_) => unchanged_count += 1,
        }
    }
    for path in left_entries.keys() {
        if !right_entries.contains_key(path) {
            removed.push(path.clone());
        }
    }
    let manifest_changed = match (
        left_entries.get("AndroidManifest.xml"),
        right_entries.get("AndroidManifest.xml"),
    ) {
        (Some(left_digest), Some(right_digest)) => Some(left_digest != right_digest),
        _ => None,
    };
    Ok(ArchiveCompareResult {
        added,
        removed,
        changed,
        unchanged_count,
        manifest_changed,
    })
}

#[tauri::command]
pub fn start_android_archive_compare(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: ArchiveCompareRequest,
) -> Result<String, String> {
    let left = archive_path(&request.left_path)?;
    let right = archive_path(&request.right_path)?;
    let manager = state.inner().clone();
    let target = format!("{} <> {}", left.display(), right.display());
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "archive-compare",
        &target,
        "正在准备深度对比...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = compare_android_archives_inner(
            &request,
            Some((&task_app, &task_id_for_thread, &cancellation)),
        );
        let detail = result.as_ref().ok().map(|result| {
            format!(
                "新增 {}，删除 {}，修改 {}，未变 {}。",
                result.added.len(),
                result.removed.len(),
                result.changed.len(),
                result.unchanged_count
            )
        });
        let error = result.as_ref().err().map(String::as_str);
        let artifact = result.as_ref().ok().and_then(|result| {
            let root = task_app
                .path()
                .app_data_dir()
                .ok()?
                .join("archive-comparisons");
            fs::create_dir_all(&root).ok()?;
            let path = root.join(format!("{}.json", task_id_for_thread));
            serde_json::to_vec(result).ok().and_then(|content| {
                fs::write(&path, content)
                    .ok()
                    .map(|_| path.to_string_lossy().to_string())
            })
        });
        let _ = android_tasks::finish_task(
            &task_app,
            &manager,
            &task_id_for_thread,
            artifact.as_deref(),
            error,
        );
        if let Some(detail) = detail {
            let _ =
                android_tasks::record_task_audit(&task_app, &task_id_for_thread, "result", &detail);
        }
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_android_archive_compare_result(
    app: AppHandle,
    artifact_path: String,
) -> Result<ArchiveCompareResult, String> {
    let root = fs::canonicalize(
        app.path()
            .app_data_dir()
            .map_err(|error| format!("无法获取应用数据目录：{error}"))?
            .join("archive-comparisons"),
    )
    .map_err(|error| format!("无法定位对比产物目录：{error}"))?;
    let path = fs::canonicalize(artifact_path.trim())
        .map_err(|error| format!("无法读取对比产物：{error}"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("对比产物不属于当前应用。".to_string());
    }
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取对比产物：{error}"))?;
    if metadata.len() > MAX_REMOTE_TEXT_BYTES {
        return Err("对比产物超过读取限制。".to_string());
    }
    let content = fs::read(&path).map_err(|error| format!("无法读取对比产物：{error}"))?;
    serde_json::from_slice(&content).map_err(|error| format!("对比产物格式无效：{error}"))
}

#[tauri::command]
pub fn inspect_android_archive(app: AppHandle, path: String) -> Result<ArchiveInspection, String> {
    inspect_android_archive_inner(&app, path, None)
}

pub(super) fn inspect_android_archive_inner(
    app: &AppHandle,
    path: String,
    task: Option<(&str, &AtomicBool)>,
) -> Result<ArchiveInspection, String> {
    let path = archive_path(&path)?;
    let kind =
        android_package_kind(&path).ok_or_else(|| "不支持的 Android 文件类型。".to_string())?;
    if kind == AndroidPackageKind::So {
        return inspect_native_shared_object(app, &path, task);
    }
    let is_aar = kind == AndroidPackageKind::Aar;
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取文件信息：{error}"))?;
    let file = File::open(&path).map_err(|error| format!("无法打开压缩包：{error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("不是可读取的 ZIP/APK/AAR/JAR 文件：{error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("压缩包条目过多，超过首版展示限制。".to_string());
    }
    let mut entries = Vec::with_capacity(archive.len());
    let mut compressed_size = 0u64;
    let mut uncompressed_size = 0u64;
    for index in 0..archive.len() {
        if let Some((task_id, cancellation)) = task {
            if cancellation.load(Ordering::Relaxed) {
                return Err("压缩包检查已取消。".to_string());
            }
            if index % 100 == 0 {
                let progress = 5 + ((index * 55) / archive.len().max(1)) as u8;
                android_tasks::update_task(
                    app,
                    task_id,
                    progress,
                    &format!("正在读取 {index}/{} 个压缩包条目...", archive.len()),
                )?;
            }
        }
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取压缩包条目：{error}"))?;
        let entry_name = entry.name().to_string();
        let entry_compressed_size = entry.compressed_size();
        let entry_uncompressed_size = entry.size();
        let is_directory = entry.is_dir();
        drop(entry);
        compressed_size = compressed_size.saturating_add(entry_compressed_size);
        uncompressed_size = uncompressed_size.saturating_add(entry_uncompressed_size);
        if should_index_embedded_aar_jar(is_aar, &entry_name, entry_uncompressed_size) {
            let mut jar_data = Vec::with_capacity(entry_uncompressed_size as usize);
            let mut classes_jar = archive
                .by_index(index)
                .map_err(|error| format!("无法读取 AAR 内的 classes.jar：{error}"))?;
            classes_jar
                .read_to_end(&mut jar_data)
                .map_err(|error| format!("无法读取 AAR 内的 classes.jar：{error}"))?;
            let mut nested = ZipArchive::new(Cursor::new(jar_data))
                .map_err(|error| format!("AAR 内的 classes.jar 无法读取：{error}"))?;
            entries.push(ArchiveEntry {
                path: "classes.jar/".to_string(),
                compressed_size: entry_compressed_size,
                uncompressed_size: entry_uncompressed_size,
                is_directory: true,
            });
            for nested_index in 0..nested.len() {
                let nested_entry = nested
                    .by_index(nested_index)
                    .map_err(|error| format!("无法读取 AAR 内嵌类文件：{error}"))?;
                entries.push(ArchiveEntry {
                    path: format!("classes.jar/{}", nested_entry.name()),
                    compressed_size: nested_entry.compressed_size(),
                    uncompressed_size: nested_entry.size(),
                    is_directory: nested_entry.is_dir(),
                });
            }
        } else {
            entries.push(ArchiveEntry {
                path: entry_name,
                compressed_size: entry_compressed_size,
                uncompressed_size: entry_uncompressed_size,
                is_directory,
            });
        }
    }
    let hash = sha256_with_task(
        &path,
        task.map(|(task_id, cancellation)| (app, task_id, cancellation, 62, 33)),
    )?;
    let inspection = ArchiveInspection {
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("archive")
            .to_string(),
        path: path.to_string_lossy().to_string(),
        format: kind.label().to_string(),
        size: metadata.len(),
        sha256: hash,
        entry_count: entries.len(),
        entries,
        compressed_size,
        uncompressed_size,
    };
    record_archive_opened(&app, &inspection)?;
    Ok(inspection)
}

#[tauri::command]
pub fn start_android_archive_inspection(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    path: String,
) -> Result<String, String> {
    start_android_archive_inspection_with_manager(app, state.inner().clone(), path)
}

pub fn start_android_archive_inspection_with_manager(
    app: AppHandle,
    manager: AndroidTaskManager,
    path: String,
) -> Result<String, String> {
    let archive = archive_path(&path)?;
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "archive-inspect",
        &archive.to_string_lossy(),
        "正在准备压缩包检查...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = inspect_android_archive_inner(
            &task_app,
            path,
            Some((&task_id_for_thread, &cancellation)),
        );
        let artifact = result.as_ref().ok().and_then(|inspection| {
            let root = task_app
                .path()
                .app_data_dir()
                .ok()?
                .join("archive-inspections");
            fs::create_dir_all(&root).ok()?;
            let path = root.join(format!("{}.json", task_id_for_thread));
            serde_json::to_vec(inspection).ok().and_then(|content| {
                fs::write(&path, content)
                    .ok()
                    .map(|_| path.to_string_lossy().to_string())
            })
        });
        let error = result.as_ref().err().map(String::as_str);
        let _ = android_tasks::finish_task(
            &task_app,
            &manager,
            &task_id_for_thread,
            artifact.as_deref(),
            error,
        );
    });
    Ok(task_id)
}

#[tauri::command]
pub fn read_android_archive_inspection(
    app: AppHandle,
    artifact_path: String,
) -> Result<ArchiveInspection, String> {
    let root = fs::canonicalize(
        app.path()
            .app_data_dir()
            .map_err(|error| format!("无法获取应用数据目录：{error}"))?
            .join("archive-inspections"),
    )
    .map_err(|error| format!("无法定位检查产物目录：{error}"))?;
    let path = fs::canonicalize(artifact_path.trim())
        .map_err(|error| format!("无法读取检查产物：{error}"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("压缩包检查产物不属于当前应用。".to_string());
    }
    let metadata = fs::metadata(&path).map_err(|error| format!("无法读取检查产物：{error}"))?;
    if metadata.len() > 64 * 1024 * 1024 {
        return Err("压缩包检查产物超过读取限制。".to_string());
    }
    let content = fs::read(&path).map_err(|error| format!("无法读取检查产物：{error}"))?;
    serde_json::from_slice(&content).map_err(|error| format!("压缩包检查产物格式无效：{error}"))
}

#[tauri::command]
pub async fn search_android_archive_entries(
    request: ArchiveSearchRequest,
) -> Result<ArchiveSearchResult, String> {
    tauri::async_runtime::spawn_blocking(move || search_android_archive_entries_inner(&request))
        .await
        .map_err(|error| format!("包内容搜索任务失败：{error}"))?
}

pub(super) fn search_android_archive_entries_inner(
    request: &ArchiveSearchRequest,
) -> Result<ArchiveSearchResult, String> {
    let query = request.query.trim();
    if query.is_empty() {
        return Ok(ArchiveSearchResult {
            matches: Vec::new(),
            scanned_entries: 0,
            skipped_entries: 0,
            truncated: false,
        });
    }
    if query.chars().count() > 256 {
        return Err("搜索关键词不能超过 256 个字符。".to_string());
    }

    let source = archive_path(&request.archive_path)?;
    let kind =
        android_package_kind(&source).ok_or_else(|| "不支持的 Android 文件类型。".to_string())?;
    if kind == AndroidPackageKind::So {
        return Err("原生 SO 库暂不提供包内条目搜索。".to_string());
    }

    let file = File::open(&source).map_err(|error| format!("无法打开压缩包：{error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| format!("无法读取压缩包：{error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("压缩包条目过多，超过搜索限制。".to_string());
    }

    let query_folded = query.to_lowercase();
    let mut matches = Vec::new();
    let mut scanned_entries = 0usize;
    let mut skipped_entries = 0usize;
    let mut scanned_bytes = 0u64;
    let mut truncated = false;

    for index in 0..archive.len() {
        if matches.len() >= MAX_ARCHIVE_SEARCH_RESULTS {
            truncated = true;
            break;
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取压缩包条目：{error}"))?;
        if entry.is_dir() {
            continue;
        }
        scanned_entries += 1;
        let entry_path = entry.name().to_string();
        if entry_path.to_lowercase().contains(&query_folded)
            && !push_archive_search_match(
                &mut matches,
                &mut truncated,
                ArchiveSearchMatch {
                    entry_path: entry_path.clone(),
                    line: None,
                    column: None,
                    preview: entry_path.clone(),
                    kind: "文件路径".to_string(),
                },
            )
        {
            break;
        }

        // AAR stores application classes in a nested classes.jar. The archive
        // inspector exposes those entries as classes.jar/<entry>, so search
        // the same virtual paths here instead of treating classes.jar as an
        // opaque binary file.
        if kind == AndroidPackageKind::Aar && entry_path == "classes.jar" {
            if entry.size() > MAX_AAR_EMBEDDED_JAR_INDEX_BYTES {
                skipped_entries += 1;
                continue;
            }
            let mut jar_data = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut jar_data)
                .map_err(|error| format!("无法读取 AAR 内的 classes.jar：{error}"))?;
            let mut nested = ZipArchive::new(Cursor::new(jar_data))
                .map_err(|error| format!("AAR 内的 classes.jar 无法读取：{error}"))?;
            if nested.len() > MAX_ARCHIVE_ENTRIES {
                return Err("AAR 内嵌 classes.jar 条目过多，超过搜索限制。".to_string());
            }
            for nested_index in 0..nested.len() {
                if matches.len() >= MAX_ARCHIVE_SEARCH_RESULTS {
                    truncated = true;
                    break;
                }
                let mut nested_entry = nested
                    .by_index(nested_index)
                    .map_err(|error| format!("无法读取 AAR 内嵌类文件：{error}"))?;
                if nested_entry.is_dir() {
                    continue;
                }
                scanned_entries += 1;
                let virtual_path = format!("classes.jar/{}", nested_entry.name());
                if virtual_path.to_lowercase().contains(&query_folded)
                    && !push_archive_search_match(
                        &mut matches,
                        &mut truncated,
                        ArchiveSearchMatch {
                            entry_path: virtual_path.clone(),
                            line: None,
                            column: None,
                            preview: virtual_path.clone(),
                            kind: "文件路径".to_string(),
                        },
                    )
                {
                    break;
                }

                let Some(search_kind) = archive_search_content_kind(&virtual_path) else {
                    skipped_entries += 1;
                    continue;
                };
                let remaining = MAX_ARCHIVE_SEARCH_TOTAL_BYTES.saturating_sub(scanned_bytes);
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                let allowed = nested_entry
                    .size()
                    .min(MAX_ARCHIVE_SEARCH_ENTRY_BYTES)
                    .min(remaining);
                if allowed < nested_entry.size() {
                    truncated = true;
                }
                let mut data = Vec::with_capacity(allowed as usize);
                nested_entry
                    .by_ref()
                    .take(allowed)
                    .read_to_end(&mut data)
                    .map_err(|error| format!("无法读取 AAR 内嵌类文件：{error}"))?;
                scanned_bytes = scanned_bytes.saturating_add(data.len() as u64);

                let match_to_add = if search_kind == "文本内容" {
                    let content = String::from_utf8_lossy(&data);
                    archive_text_search_context(content.as_ref(), query).map(
                        |(line, column, preview)| ArchiveSearchMatch {
                            entry_path: virtual_path.clone(),
                            line: Some(line),
                            column: Some(column),
                            preview,
                            kind: "文本内容".to_string(),
                        },
                    )
                } else if query.is_ascii() {
                    find_ascii_case_insensitive(&data, query.as_bytes()).map(|offset| {
                        ArchiveSearchMatch {
                            entry_path: virtual_path.clone(),
                            line: None,
                            column: None,
                            preview: archive_binary_search_preview(&data, offset, query.len()),
                            kind: search_kind.to_string(),
                        }
                    })
                } else {
                    None
                };
                if let Some(next_match) = match_to_add {
                    if !push_archive_search_match(&mut matches, &mut truncated, next_match) {
                        break;
                    }
                }
            }
            continue;
        }

        if kind == AndroidPackageKind::Apk && entry_path == "AndroidManifest.xml" {
            if let Some(decoded) = decoded_manifest_preview(&source) {
                if let Some((line, column, preview)) = archive_text_search_context(&decoded, query)
                {
                    let should_continue = push_archive_search_match(
                        &mut matches,
                        &mut truncated,
                        ArchiveSearchMatch {
                            entry_path,
                            line: Some(line),
                            column: Some(column),
                            preview,
                            kind: "已解码清单".to_string(),
                        },
                    );
                    if !should_continue {
                        break;
                    }
                }
            }
            continue;
        }

        let Some(search_kind) = archive_search_content_kind(&entry_path) else {
            skipped_entries += 1;
            continue;
        };
        let remaining = MAX_ARCHIVE_SEARCH_TOTAL_BYTES.saturating_sub(scanned_bytes);
        if remaining == 0 {
            truncated = true;
            break;
        }
        let allowed = entry
            .size()
            .min(MAX_ARCHIVE_SEARCH_ENTRY_BYTES)
            .min(remaining);
        if allowed < entry.size() {
            truncated = true;
        }
        let mut data = Vec::with_capacity(allowed as usize);
        entry
            .by_ref()
            .take(allowed)
            .read_to_end(&mut data)
            .map_err(|error| format!("无法读取包内文件：{error}"))?;
        scanned_bytes = scanned_bytes.saturating_add(data.len() as u64);

        let match_to_add = if search_kind == "文本内容" {
            let content = String::from_utf8_lossy(&data);
            archive_text_search_context(content.as_ref(), query).map(|(line, column, preview)| {
                ArchiveSearchMatch {
                    entry_path,
                    line: Some(line),
                    column: Some(column),
                    preview,
                    kind: "文本内容".to_string(),
                }
            })
        } else if query.is_ascii() {
            find_ascii_case_insensitive(&data, query.as_bytes()).map(|offset| ArchiveSearchMatch {
                entry_path,
                line: None,
                column: None,
                preview: archive_binary_search_preview(&data, offset, query.len()),
                kind: search_kind.to_string(),
            })
        } else {
            None
        };
        if let Some(next_match) = match_to_add {
            if !push_archive_search_match(&mut matches, &mut truncated, next_match) {
                break;
            }
        }
    }

    Ok(ArchiveSearchResult {
        matches,
        scanned_entries,
        skipped_entries,
        truncated,
    })
}

pub(super) fn push_archive_search_match(
    matches: &mut Vec<ArchiveSearchMatch>,
    truncated: &mut bool,
    next_match: ArchiveSearchMatch,
) -> bool {
    if matches.len() >= MAX_ARCHIVE_SEARCH_RESULTS {
        *truncated = true;
        return false;
    }
    matches.push(next_match);
    true
}

pub(super) fn archive_search_content_kind(entry_path: &str) -> Option<&'static str> {
    let lower = entry_path.to_ascii_lowercase();
    if lower.ends_with(".dex") {
        return Some("DEX 字符串");
    }
    if lower.ends_with(".class") {
        return Some("Class 常量");
    }
    let extension = Path::new(&lower).extension()?.to_str()?;
    [
        "xml",
        "json",
        "txt",
        "properties",
        "mf",
        "pro",
        "gradle",
        "java",
        "kt",
        "kts",
        "smali",
        "aidl",
        "html",
        "htm",
        "js",
        "ts",
        "css",
        "c",
        "cc",
        "cpp",
        "h",
        "hpp",
        "rs",
        "toml",
        "yaml",
        "yml",
        "md",
    ]
    .contains(&extension)
    .then_some("文本内容")
}

pub(super) fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.to_ascii_lowercase() == right.to_ascii_lowercase())
    })
}

pub(super) fn archive_text_search_context(
    content: &str,
    query: &str,
) -> Option<(usize, usize, String)> {
    let offset = if query.is_ascii() {
        find_ascii_case_insensitive(content.as_bytes(), query.as_bytes())?
    } else {
        content
            .find(query)
            .or_else(|| content.to_lowercase().find(&query.to_lowercase()))?
    };
    if !content.is_char_boundary(offset) {
        return None;
    }
    let prefix = &content[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let line_end = content[offset..]
        .find('\n')
        .map_or(content.len(), |index| offset + index);
    let column = content[line_start..offset].chars().count() + 1;
    Some((
        line,
        column,
        archive_search_preview(&content[line_start..line_end]),
    ))
}

pub(super) fn archive_search_preview(value: &str) -> String {
    let mut preview = value
        .chars()
        .map(|character| {
            if character.is_control() && character != '\t' {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    const MAX_PREVIEW_CHARS: usize = 220;
    if preview.chars().count() > MAX_PREVIEW_CHARS {
        preview = preview.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
        preview.push_str("...");
    }
    preview
}

pub(super) fn archive_binary_search_preview(
    data: &[u8],
    offset: usize,
    query_length: usize,
) -> String {
    let start = offset.saturating_sub(56);
    let end = (offset + query_length + 112).min(data.len());
    let preview = data[start..end]
        .iter()
        .map(|byte| match byte {
            b'\t' | b'\n' | b'\r' => ' ',
            0x20..=0x7e => *byte as char,
            _ => ' ',
        })
        .collect::<String>();
    archive_search_preview(&preview)
}

#[tauri::command]
pub fn preview_android_archive_entry(
    request: ArchiveEntryPreviewRequest,
) -> Result<ArchiveEntryPreview, String> {
    let source = archive_path(&request.archive_path)?;
    let is_manifest = request
        .entry_path
        .eq_ignore_ascii_case("AndroidManifest.xml")
        && source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"));
    if is_manifest {
        if let Some(content) = decoded_manifest_preview(&source) {
            let truncated = content.len() > MAX_PREVIEW_BYTES;
            let content = if truncated {
                let mut end = MAX_PREVIEW_BYTES;
                while !content.is_char_boundary(end) {
                    end -= 1;
                }
                content[..end].to_string()
            } else {
                content
            };
            return Ok(ArchiveEntryPreview {
                path: request.entry_path,
                size: content.len() as u64,
                kind: "text".to_string(),
                content,
                truncated,
            });
        }
    }
    if let Some((size, data)) = aar_embedded_entry(&source, &request.entry_path) {
        if request.entry_path.to_ascii_lowercase().ends_with(".class") {
            if let Some(content) = decompile_class_bytes(&data, &request.entry_path) {
                let truncated = content.len() > MAX_PREVIEW_BYTES;
                let content = if truncated {
                    let mut end = MAX_PREVIEW_BYTES;
                    while !content.is_char_boundary(end) {
                        end -= 1;
                    }
                    content[..end].to_string()
                } else {
                    content
                };
                return Ok(ArchiveEntryPreview {
                    path: request.entry_path,
                    size,
                    kind: "java".to_string(),
                    content,
                    truncated,
                });
            }
        }
        return Ok(preview_from_bytes(request.entry_path, size, data));
    }
    if request.entry_path.to_ascii_lowercase().ends_with(".class") {
        if let Some(content) = decompile_class_entry(&source, &request.entry_path) {
            let truncated = content.len() > MAX_PREVIEW_BYTES;
            let content = if truncated {
                let mut end = MAX_PREVIEW_BYTES;
                while !content.is_char_boundary(end) {
                    end -= 1;
                }
                content[..end].to_string()
            } else {
                content
            };
            return Ok(ArchiveEntryPreview {
                path: request.entry_path,
                size: content.len() as u64,
                kind: "java".to_string(),
                content,
                truncated,
            });
        }
    }
    let file = File::open(&source).map_err(|error| format!("无法打开压缩包：{error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| format!("无法读取压缩包：{error}"))?;
    let mut entry = archive
        .by_name(&request.entry_path)
        .map_err(|error| format!("无法读取包内文件：{error}"))?;
    if entry.is_dir() {
        return Err("目录没有可预览的文件内容。".to_string());
    }
    let size = entry.size();
    let mut data = Vec::with_capacity((size as usize).min(MAX_PREVIEW_BYTES));
    entry
        .by_ref()
        .take((MAX_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|error| format!("无法读取包内文件：{error}"))?;
    Ok(preview_from_bytes(request.entry_path, size, data))
}

#[tauri::command]
pub fn extract_android_archive(
    request: ArchiveExtractRequest,
) -> Result<ArchiveExtractResult, String> {
    extract_android_archive_inner(&request, None)
}

pub(super) fn extract_android_archive_inner(
    request: &ArchiveExtractRequest,
    task: Option<(&AppHandle, &str, &AtomicBool)>,
) -> Result<ArchiveExtractResult, String> {
    let source = archive_path(&request.archive_path)?;
    let destination = PathBuf::from(request.destination.trim());
    if destination.as_os_str().is_empty() {
        return Err("请选择解包目录。".to_string());
    }
    fs::create_dir_all(&destination).map_err(|error| format!("无法创建解包目录：{error}"))?;
    let destination =
        fs::canonicalize(&destination).map_err(|error| format!("无法访问解包目录：{error}"))?;
    let file = File::open(source).map_err(|error| format!("无法打开压缩包：{error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| format!("无法读取压缩包：{error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("压缩包条目过多，超过首版解包限制。".to_string());
    }
    let mut files_extracted = 0usize;
    for index in 0..archive.len() {
        if let Some((app, task_id, cancellation)) = task {
            if cancellation.load(Ordering::Relaxed) {
                return Err("解包已取消。".to_string());
            }
            if index % 100 == 0 {
                let progress = 5 + ((index * 90) / archive.len().max(1)) as u8;
                android_tasks::update_task(
                    app,
                    task_id,
                    progress,
                    &format!("正在解包 {index}/{} 个条目...", archive.len()),
                )?;
            }
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取压缩包条目：{error}"))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("压缩包包含不安全路径：{}", entry.name()))?
            .to_path_buf();
        let target = destination.join(relative);
        if !target.starts_with(&destination) {
            return Err("压缩包路径越界。".to_string());
        }
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|error| format!("无法创建目录：{error}"))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建目录：{error}"))?;
        }
        if target.exists() {
            return Err(format!(
                "解包目标已存在，已停止以避免覆盖文件：{}",
                target.display()
            ));
        }
        let mut output =
            File::create(&target).map_err(|error| format!("无法创建解包文件：{error}"))?;
        std::io::copy(&mut entry, &mut output).map_err(|error| format!("无法解包文件：{error}"))?;
        output
            .flush()
            .map_err(|error| format!("无法写入解包文件：{error}"))?;
        files_extracted += 1;
    }
    Ok(ArchiveExtractResult {
        destination: destination.to_string_lossy().to_string(),
        files_extracted,
    })
}

#[tauri::command]
pub fn start_android_archive_extract(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: ArchiveExtractRequest,
) -> Result<String, String> {
    let source = archive_path(&request.archive_path)?;
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "archive-extract",
        &source.to_string_lossy(),
        "正在准备解包...",
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = extract_android_archive_inner(
            &request,
            Some((&task_app, &task_id_for_thread, &cancellation)),
        );
        let artifact = result
            .as_ref()
            .ok()
            .map(|result| result.destination.as_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

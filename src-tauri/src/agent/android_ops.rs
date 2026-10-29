use super::*;

pub(super) struct AndroidArchiveReadTool {
    pub(super) app: AppHandle,
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}

pub(super) struct AndroidArchiveWriteTool {
    pub(super) app: AppHandle,
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}

pub(super) struct AndroidAdbReadTool {
    pub(super) app: AppHandle,
    pub(super) cancellation: Arc<AtomicBool>,
}

pub(super) struct AndroidAdbActionTool {
    pub(super) app: AppHandle,
    pub(super) roots: Vec<String>,
    pub(super) allow_external_paths: bool,
    pub(super) cancellation: Arc<AtomicBool>,
}

fn string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少必填参数 {key}"))
}

fn allowed_file_path(
    input: &Value,
    key: &str,
    roots: &[String],
    allow_external_paths: bool,
) -> Result<String, String> {
    let raw = string(input, key)?;
    let path = PathBuf::from(&raw);
    if !is_allowed_agent_path(&path, roots, allow_external_paths) || !path.is_file() {
        return Err(format!("{key} 必须是当前已授权工作区中的现有文件"));
    }
    Ok(path.to_string_lossy().to_string())
}

fn allowed_directory(
    input: &Value,
    key: &str,
    roots: &[String],
    allow_external_paths: bool,
) -> Result<String, String> {
    let raw = string(input, key)?;
    let path = fs::canonicalize(&raw).map_err(|error| format!("无法访问 {key}：{error}"))?;
    if !path.is_dir() || !is_allowed_agent_path(&path, roots, allow_external_paths) {
        return Err(format!("{key} 必须是当前已授权工作区中的目录"));
    }
    Ok(path.to_string_lossy().to_string())
}

fn allowed_output_path(
    input: &Value,
    key: &str,
    roots: &[String],
    allow_external_paths: bool,
) -> Result<String, String> {
    let raw = string(input, key)?;
    let target = PathBuf::from(&raw);
    let parent = if target.is_dir() {
        target.as_path()
    } else {
        target.parent().ok_or_else(|| format!("{key} 路径无效"))?
    };
    let parent =
        fs::canonicalize(parent).map_err(|error| format!("无法访问 {key} 的父目录：{error}"))?;
    if !parent.is_dir() || !is_allowed_agent_path(&parent, roots, allow_external_paths) {
        return Err(format!("{key} 必须位于当前已授权工作区中"));
    }
    if target.exists() {
        let resolved =
            fs::canonicalize(&target).map_err(|error| format!("无法访问 {key}：{error}"))?;
        if !is_allowed_agent_path(&resolved, roots, allow_external_paths) {
            return Err(format!("{key} 不能通过符号链接指向授权工作区之外"));
        }
    }
    Ok(target.to_string_lossy().to_string())
}

fn as_json<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| format!("无法格式化 Android 工具结果：{error}"))
}

fn managed_jadx_workspace(app: &AppHandle, input: &Value) -> Result<String, String> {
    let workspace = fs::canonicalize(string(input, "workspacePath")?)
        .map_err(|error| format!("无法访问 JADX 工作区：{error}"))?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("jadx-workspaces");
    let root = fs::canonicalize(root).map_err(|error| format!("JADX 工作区尚未创建：{error}"))?;
    if !workspace.starts_with(root) {
        return Err("JADX 工作区不属于当前应用".to_string());
    }
    Ok(workspace.to_string_lossy().to_string())
}

fn bounded_text(mut text: String, limit: usize) -> Value {
    let truncated = text.len() > limit;
    if truncated {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    json!({"content":text,"truncated":truncated})
}

fn bounded_array<T: serde::Serialize>(items: Vec<T>, limit: usize) -> Result<Value, String> {
    let total = items.len();
    let items = items.into_iter().take(limit).collect::<Vec<_>>();
    Ok(json!({"items":items,"total":total,"truncated":total > limit}))
}

fn check_cancelled(cancellation: &AtomicBool) -> Result<(), String> {
    if cancellation.load(Ordering::Relaxed) {
        Err("请求已取消".to_string())
    } else {
        Ok(())
    }
}

#[async_trait]
impl ToolHandler for AndroidArchiveReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "android_archive_read",
            "直接查询 APK、AAB、APKS、AAR、JAR、ZIP、SO 的结构和内容，不打开归档页面。支持 inspect、search、preview、compare、JADX/workspace 浏览与历史查询。文件路径必须来自用户已授权的工作区。",
            json!({
                "type":"object",
                "properties": {
                    "action":{"type":"string","enum":["inspect","search","preview","compare","jadx_workspace","jadx_source","apk_workspace_files","apk_workspace_file","apk_workspace_diff","original_manifest","build_result","history"]},
                    "path":{"type":"string","description":"已授权工作区中的归档文件路径"},
                    "otherPath":{"type":"string","description":"compare 的第二个归档文件路径"},
                    "query":{"type":"string","description":"search 的关键词"},
                    "entryPath":{"type":"string","description":"preview 的压缩包内相对路径"},
                    "workspacePath":{"type":"string","description":"已生成的 JADX 工作区路径"},
                    "relativePath":{"type":"string","description":"工作区内相对路径"},
                    "sessionId":{"type":"string","description":"已创建的 APK 编辑会话 ID"},
                    "buildId":{"type":"string","description":"已完成的 APK 构建 ID"},
                    "historyId":{"type":"string","description":"可选的归档历史 ID"}
                },
                "required":["action"]
            }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        check_cancelled(&self.cancellation)?;
        let action = string(&input, "action")?;
        let archive_path =
            || allowed_file_path(&input, "path", &self.roots, self.allow_external_paths);
        match action.as_str() {
            "inspect" => {
                let mut inspection = as_json(crate::android_tools::inspect_android_archive(
                    self.app.clone(),
                    archive_path()?,
                )?)?;
                if let Some(entries) = inspection.get_mut("entries").and_then(Value::as_array_mut) {
                    let total = entries.len();
                    entries.truncate(200);
                    inspection["entriesTruncated"] = json!(total > 200);
                }
                Ok(inspection)
            }
            "search" => as_json(
                crate::android_tools::search_android_archive_entries(
                    crate::android_tools::ArchiveSearchRequest {
                        archive_path: archive_path()?,
                        query: string(&input, "query")?,
                    },
                )
                .await?,
            ),
            "preview" => as_json(crate::android_tools::preview_android_archive_entry(
                crate::android_tools::ArchiveEntryPreviewRequest {
                    archive_path: archive_path()?,
                    entry_path: string(&input, "entryPath")?,
                },
            )?),
            "compare" => {
                let mut result = as_json(crate::android_tools::compare_android_archives(
                    crate::android_tools::ArchiveCompareRequest {
                        left_path: archive_path()?,
                        right_path: allowed_file_path(
                            &json!({"path": string(&input, "otherPath")?}),
                            "path",
                            &self.roots,
                            self.allow_external_paths,
                        )?,
                    },
                )?)?;
                for key in ["added", "removed", "changed"] {
                    if let Some(entries) = result.get_mut(key).and_then(Value::as_array_mut) {
                        let total = entries.len();
                        entries.truncate(200);
                        result[format!("{key}Truncated")] = json!(total > 200);
                    }
                }
                Ok(result)
            }
            "jadx_workspace" => {
                let mut result = as_json(crate::android_tools::inspect_jadx_workspace(
                    managed_jadx_workspace(&self.app, &input)?,
                )?)?;
                if let Some(files) = result.get_mut("files").and_then(Value::as_array_mut) {
                    let total = files.len();
                    files.truncate(500);
                    result["filesTruncated"] = json!(total > 500);
                }
                Ok(result)
            }
            "jadx_source" => Ok(bounded_text(
                crate::android_tools::read_jadx_source(crate::android_tools::JadxSourceRequest {
                    workspace_path: managed_jadx_workspace(&self.app, &input)?,
                    relative_path: string(&input, "relativePath")?,
                })?,
                64 * 1024,
            )),
            "apk_workspace_files" => bounded_array(
                crate::android_tools::list_apk_workspace_files(
                    self.app.clone(),
                    string(&input, "sessionId")?,
                )?,
                500,
            ),
            "apk_workspace_file" => {
                let diff = crate::android_tools::read_apk_workspace_file(
                    self.app.clone(),
                    crate::android_tools::WorkspaceFileRequest {
                        session_id: string(&input, "sessionId")?,
                        relative_path: string(&input, "relativePath")?,
                    },
                )?;
                Ok(json!({
                    "original":bounded_text(diff.original, 64 * 1024),
                    "current":bounded_text(diff.current, 64 * 1024)
                }))
            }
            "apk_workspace_diff" => as_json(crate::android_tools::summarize_apk_workspace_diff(
                self.app.clone(),
                string(&input, "sessionId")?,
            )?),
            "original_manifest" => Ok(
                json!({"manifest":crate::android_tools::read_archive_original_manifest(
                self.app.clone(),
                string(&input, "sessionId")?,
            )?}),
            ),
            "build_result" => as_json(crate::android_tools::read_archive_build_result(
                self.app.clone(),
                string(&input, "buildId")?,
            )?),
            "history" => {
                let history = crate::android_tools::list_archive_history(self.app.clone())?;
                let requested = input.get("historyId").and_then(Value::as_str);
                let records = history
                    .into_iter()
                    .filter(|record| requested.map_or(true, |id| record.id == id))
                    .take(100)
                    .collect::<Vec<_>>();
                as_json(records)
            }
            _ => Err("不支持的归档只读操作".to_string()),
        }
    }
}

#[async_trait]
impl ToolHandler for AndroidArchiveWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "android_archive_edit",
            "对用户已授权的 Android 归档文件执行解包、创建 APK 编辑工作区、JADX 反编译、编辑工作区文件、构建或安装 APK。所有操作都直接调用归档后端并记录在应用历史中；安装及磁盘写入需要用户确认。",
            json!({
                "type":"object",
                "properties": {
                    "action":{"type":"string","enum":["extract","begin_apk_edit","jadx_decompile","write_workspace_file","build_signed_apk","install_built_apk","build_apks_from_aab","install_apks","delete_history"]},
                    "path":{"type":"string"},
                    "destination":{"type":"string"},
                    "sessionId":{"type":"string"},
                    "relativePath":{"type":"string"},
                    "content":{"type":"string"},
                    "manifest":{"type":"string"},
                    "buildId":{"type":"string"},
                    "serial":{"type":"string"},
                    "replace":{"type":"boolean"},
                    "historyId":{"type":"string"},
                    "deleteWorkspace":{"type":"boolean"}
                },
                "required":["action"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        check_cancelled(&self.cancellation)?;
        let action = string(&input, "action")?;
        let archive_path =
            || allowed_file_path(&input, "path", &self.roots, self.allow_external_paths);
        match action.as_str() {
            "extract" => {
                let result = crate::android_tools::extract_android_archive(
                    crate::android_tools::ArchiveExtractRequest {
                        archive_path: archive_path()?,
                        destination: allowed_output_path(
                            &input,
                            "destination",
                            &self.roots,
                            self.allow_external_paths,
                        )?,
                    },
                )?;
                as_json(result)
            }
            "begin_apk_edit" => as_json(crate::android_tools::begin_apk_edit_session(
                self.app.clone(),
                crate::android_tools::ArchiveEditSessionRequest {
                    archive_path: archive_path()?,
                },
            )?),
            "jadx_decompile" => as_json(crate::android_tools::decompile_apk_with_jadx(
                self.app.clone(),
                crate::android_tools::JadxDecompileRequest {
                    archive_path: archive_path()?,
                },
            )?),
            "write_workspace_file" => {
                crate::android_tools::write_apk_workspace_file(
                    self.app.clone(),
                    crate::android_tools::WorkspaceWriteRequest {
                        session_id: string(&input, "sessionId")?,
                        relative_path: string(&input, "relativePath")?,
                        content: input
                            .get("content")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "缺少必填参数 content".to_string())?
                            .to_string(),
                    },
                )?;
                Ok(json!({"written":true}))
            }
            "build_signed_apk" => as_json(crate::android_tools::build_signed_apk(
                self.app.clone(),
                crate::android_tools::ArchiveBuildRequest {
                    session_id: string(&input, "sessionId")?,
                    manifest: input
                        .get("manifest")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    destination: allowed_output_path(
                        &input,
                        "destination",
                        &self.roots,
                        self.allow_external_paths,
                    )?,
                },
            )?),
            "install_built_apk" => as_json(crate::android_tools::install_archive_build(
                self.app.clone(),
                crate::android_tools::ArchiveBuildInstallRequest {
                    session_id: string(&input, "sessionId")?,
                    serial: string(&input, "serial")?,
                    replace: input
                        .get("replace")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
            )?),
            "build_apks_from_aab" => Ok(
                json!({"outputPath":crate::android_tools::build_apks_from_aab(
                self.app.clone(),
                crate::android_tools::BundletoolBuildRequest {
                    aab_path: archive_path()?,
                    destination: allowed_output_path(
                        &input,
                        "destination",
                        &self.roots,
                        self.allow_external_paths,
                    )?,
                },
            )?}),
            ),
            "install_apks" => Ok(
                json!({"result":crate::android_tools::install_apks_with_bundletool(
                self.app.clone(),
                crate::android_tools::BundletoolInstallRequest {
                    serial: string(&input, "serial")?,
                    apks_path: allowed_file_path(
                        &json!({"path":string(&input,"apksPath")?}),
                        "path",
                        &self.roots,
                        self.allow_external_paths,
                    )?,
                },
            )?}),
            ),
            "delete_history" => {
                crate::android_tools::delete_archive_history(
                    self.app.clone(),
                    crate::android_tools::ArchiveHistoryDeleteRequest {
                        id: string(&input, "historyId")?,
                        delete_workspace: input
                            .get("deleteWorkspace")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    },
                )?;
                Ok(json!({"deleted":true}))
            }
            _ => Err("不支持的归档写入操作".to_string()),
        }
    }
}

#[async_trait]
impl ToolHandler for AndroidAdbReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "android_adb_read",
            "通过现有 ADB 后端执行只读查询：设备、应用包、进程、包详情、设备文件、文本、备份、存储与权限状态。只返回查询结果，不运行任意 shell 命令。",
            json!({
                "type":"object",
                "properties": {
                    "action":{"type":"string","enum":["devices","device","packages","applications","processes","package_details","remote_files","read_remote_text","backups","storage","capture_info","privilege"]},
                    "serial":{"type":"string"},"packageName":{"type":"string"},"path":{"type":"string"}
                },
                "required":["action"]
            }),
        )
        .read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        check_cancelled(&self.cancellation)?;
        let action = string(&input, "action")?;
        let serial = || string(&input, "serial");
        match action.as_str() {
            "devices" => as_json(crate::android_tools::list_adb_devices()?),
            "device" => {
                let serial = serial()?;
                let (info, status) = futures_util::future::join(
                    async { crate::android_tools::get_adb_device_info(serial.clone()) },
                    async { crate::android_tools::get_adb_device_status(serial.clone()) },
                )
                .await;
                Ok(json!({"serial":serial,"info":info?,"status":status?}))
            }
            "packages" => bounded_array(crate::android_tools::list_adb_packages(serial()?)?, 500),
            "applications" => {
                bounded_array(crate::android_tools::list_adb_applications(serial()?)?, 500)
            }
            "processes" => bounded_array(
                crate::android_tools::list_adb_process_details(serial()?)?,
                500,
            ),
            "package_details" => as_json(crate::android_tools::get_adb_package_details(
                serial()?,
                string(&input, "packageName")?,
            )?),
            "remote_files" => bounded_array(
                crate::android_tools::list_adb_remote_files(serial()?, string(&input, "path")?)?,
                500,
            ),
            "read_remote_text" => {
                let text =
                    crate::android_tools::read_adb_remote_text(serial()?, string(&input, "path")?)?;
                Ok(json!({
                    "content":bounded_text(text.content, 64 * 1024),
                    "size":text.size
                }))
            }
            "backups" => bounded_array(
                crate::android_tools::list_adb_backups(self.app.clone(), serial()?)?,
                200,
            ),
            "storage" => as_json(crate::android_tools::get_adb_storage_info(
                serial()?,
                input
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("/sdcard")
                    .to_string(),
            )?),
            "capture_info" => as_json(crate::android_tools::get_adb_capture_info(serial()?)?),
            "privilege" => as_json(crate::android_tools::get_adb_privilege_status(serial()?)?),
            _ => Err("不支持的 ADB 只读操作".to_string()),
        }
    }
}

#[async_trait]
impl ToolHandler for AndroidAdbActionTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "android_adb_action",
            "通过受限的 Android ADB 接口执行明确的设备操作：无线配对/连接、应用启动/停止/清数据/卸载/启停、APK 安装导出、文件传输/写入/删除/备份恢复、Root/remount、重启和设备按键。不会开放任意 shell。每次调用需要用户授权。",
            json!({
                "type":"object",
                "properties": {
                    "action":{"type":"string","enum":["pair","connect","disconnect","restart_adb","shell","package_action","install_apk","export_apk","push_file","pull_file","write_remote_text","create_directory","delete_remote_path","restore_backup","request_root","remount","reboot","device_action","screenshot","start_screen_record","stop_screen_record"]},
                    "serial":{"type":"string"},"endpoint":{"type":"string"},"pairingCode":{"type":"string"},
                    "packageName":{"type":"string"},"packageAction":{"type":"string","enum":["launch","force-stop","clear-data","uninstall","disable","enable"]},
                    "apkPath":{"type":"string"},"destination":{"type":"string"},"localPath":{"type":"string"},"remotePath":{"type":"string"},"apksPath":{"type":"string"},
                    "path":{"type":"string"},"content":{"type":"string"},"confirmed":{"type":"boolean"},"isDirectory":{"type":"boolean"},
                    "command":{"type":"string","description":"在 Android 设备端 sh -c 执行的一条命令；不要传入主机命令或 adb 前缀"},"timeoutMs":{"type":"integer","minimum":100,"maximum":120000},
                    "backupId":{"type":"string"},"replace":{"type":"boolean"},"allowTest":{"type":"boolean"},
                    "mode":{"type":"string","enum":["normal","recovery","bootloader"]},"deviceAction":{"type":"string","enum":["wake","sleep","power","home","back","app-switch","volume-up","volume-down","vol-mute","rotate"]}
                },
                "required":["action"]
            }),
        )
        .requiring_permission()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        check_cancelled(&self.cancellation)?;
        let action = string(&input, "action")?;
        let serial = || string(&input, "serial");
        let request = |serial: String, remote_path: String, local_path: String| {
            crate::android_tools::AdbTransferRequest {
                serial,
                remote_path,
                local_path,
            }
        };
        match action.as_str() {
            "pair" => Ok(json!({"result":crate::android_tools::pair_adb_wireless(
                self.app.clone(), crate::android_tools::AdbWirelessPairRequest {
                    endpoint:string(&input,"endpoint")?, pairing_code:string(&input,"pairingCode")?,
                }
            )?})),
            "connect" | "disconnect" => {
                let req = crate::android_tools::AdbWirelessConnectRequest {
                    endpoint: string(&input, "endpoint")?,
                };
                let result = if action == "connect" {
                    crate::android_tools::connect_adb_wireless(self.app.clone(), req)?
                } else {
                    crate::android_tools::disconnect_adb_wireless(self.app.clone(), req)?
                };
                Ok(json!({"result":result}))
            }
            "restart_adb" => {
                crate::android_tools::restart_adb(self.app.clone())?;
                Ok(json!({"started":true}))
            }
            "shell" => as_json(crate::android_tools::run_adb_shell(
                self.app.clone(),
                serial()?,
                string(&input, "command")?,
                input.get("timeoutMs").and_then(Value::as_u64),
            )?),
            "start_screen_record" => Ok(
                json!({"taskId":crate::android_tools::start_adb_screen_record(
                self.app.clone(),
                self.app.state::<crate::android_tools::AndroidToolState>(),
                self.app.state::<crate::android_tasks::AndroidTaskManager>(),
                serial()?,
            )?}),
            ),
            "stop_screen_record" => Ok(
                json!({"result":crate::android_tools::stop_adb_screen_record(
                self.app.clone(),
                self.app.state::<crate::android_tools::AndroidToolState>(),
                self.app.state::<crate::android_tasks::AndroidTaskManager>(),
                crate::android_tools::AdbScreenRecordStopRequest {
                    serial: serial()?,
                    destination: allowed_output_path(
                        &json!({"destination":string(&input,"destination")?}),
                        "destination",
                        &self.roots,
                        self.allow_external_paths,
                    )?,
                },
            )?}),
            ),
            "package_action" => Ok(
                json!({"result":crate::android_tools::run_adb_package_action(
                self.app.clone(), serial()?, string(&input,"packageName")?, string(&input,"packageAction")?,
            )?}),
            ),
            "install_apk" => Ok(json!({"result":crate::android_tools::install_adb_package(
                self.app.clone(), crate::android_tools::AdbPackageInstallRequest {
                    serial:serial()?, apk_path:allowed_file_path(&input,"apkPath",&self.roots,self.allow_external_paths)?,
                    replace:input.get("replace").and_then(Value::as_bool).unwrap_or(false),
                    allow_test:input.get("allowTest").and_then(Value::as_bool).unwrap_or(false),
                }
            )?})),
            "export_apk" => as_json(crate::android_tools::export_adb_package(
                self.app.clone(),
                crate::android_tools::AdbPackageExportRequest {
                    serial: serial()?,
                    package_name: string(&input, "packageName")?,
                    destination: allowed_directory(
                        &input,
                        "destination",
                        &self.roots,
                        self.allow_external_paths,
                    )?,
                },
            )?),
            "push_file" | "pull_file" | "screenshot" => {
                let local_path = if action == "pull_file" {
                    allowed_directory(&input, "localPath", &self.roots, self.allow_external_paths)?
                } else if action == "push_file" {
                    allowed_file_path(&input, "localPath", &self.roots, self.allow_external_paths)?
                } else {
                    allowed_output_path(
                        &input,
                        "localPath",
                        &self.roots,
                        self.allow_external_paths,
                    )?
                };
                let req = request(
                    serial()?,
                    input
                        .get("remotePath")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    local_path,
                );
                let result = match action.as_str() {
                    "push_file" => crate::android_tools::push_adb_file(self.app.clone(), req)?,
                    "pull_file" => crate::android_tools::pull_adb_file(self.app.clone(), req)?,
                    _ => crate::android_tools::capture_adb_screenshot(self.app.clone(), req)?,
                };
                Ok(json!({"result":result}))
            }
            "write_remote_text" => {
                crate::android_tools::write_adb_remote_text(
                    self.app.clone(),
                    crate::android_tools::AdbWriteTextRequest {
                        serial: serial()?,
                        path: string(&input, "path")?,
                        content: input
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        confirm_protected: input
                            .get("confirmed")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    },
                )?;
                Ok(json!({"written":true}))
            }
            "create_directory" => {
                crate::android_tools::create_adb_directory(
                    self.app.clone(),
                    crate::android_tools::AdbRemotePathRequest {
                        serial: serial()?,
                        path: string(&input, "path")?,
                    },
                )?;
                Ok(json!({"created":true}))
            }
            "delete_remote_path" => as_json(crate::android_tools::delete_adb_remote_path(
                self.app.clone(),
                crate::android_tools::AdbDeleteRequest {
                    serial: serial()?,
                    path: string(&input, "path")?,
                    is_directory: input
                        .get("isDirectory")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    confirmed: input
                        .get("confirmed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
            )?),
            "restore_backup" => {
                crate::android_tools::restore_adb_backup(
                    self.app.clone(),
                    crate::android_tools::AdbBackupRestoreRequest {
                        serial: serial()?,
                        id: string(&input, "backupId")?,
                    },
                )?;
                Ok(json!({"restored":true}))
            }
            "request_root" => as_json(crate::android_tools::request_adb_root(
                self.app.clone(),
                serial()?,
            )?),
            "remount" => Ok(
                json!({"result":crate::android_tools::remount_adb_system(self.app.clone(), serial()?)?}),
            ),
            "reboot" => {
                crate::android_tools::restart_adb_device(
                    self.app.clone(),
                    serial()?,
                    input
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("normal")
                        .to_string(),
                )?;
                Ok(json!({"rebooting":true}))
            }
            "device_action" => Ok(json!({"result":crate::android_tools::run_adb_device_action(
                self.app.clone(), serial()?, string(&input,"deviceAction")?,
            )?})),
            _ => Err("不支持的 ADB 操作".to_string()),
        }
    }
}

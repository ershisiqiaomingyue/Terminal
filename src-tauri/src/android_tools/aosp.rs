use super::*;

pub(super) fn scan_aosp_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<String>,
    envsetup: &mut Option<String>,
    soong: &mut Option<String>,
    truncated: &mut bool,
) -> Result<(), String> {
    if files.len() >= 500 || *truncated {
        *truncated = true;
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(|error| format!("无法读取工作区目录：{error}"))?
    {
        let path = entry
            .map_err(|error| format!("无法读取工作区目录：{error}"))?
            .path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if matches!(name, ".git" | "out" | "node_modules" | ".repo") {
                continue;
            }
            scan_aosp_files(root, &path, files, envsetup, soong, truncated)?;
        } else if name == "Android.bp" {
            files.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else if name == "envsetup.sh"
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|parent| parent == "build")
        {
            *envsetup = Some(path.to_string_lossy().to_string());
        } else if name == "soong_ui.bash" {
            *soong = Some(path.to_string_lossy().to_string());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn inspect_aosp_workspace(root: String) -> Result<AospWorkspaceInfo, String> {
    let root = fs::canonicalize(root.trim()).map_err(|error| format!("无法读取工作区：{error}"))?;
    if !root.is_dir() {
        return Err("AOSP 工作区必须是目录。".to_string());
    }
    let mut android_bp_files = Vec::new();
    let mut envsetup_path = None;
    let mut soong_path = None;
    let mut truncated = false;
    scan_aosp_files(
        &root,
        &root,
        &mut android_bp_files,
        &mut envsetup_path,
        &mut soong_path,
        &mut truncated,
    )?;
    Ok(AospWorkspaceInfo {
        root: root.to_string_lossy().to_string(),
        envsetup_path,
        soong_path,
        android_bp_files,
        truncated,
    })
}

pub(super) fn aosp_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn aosp_target_label(target: &AospLinuxTargetRequest) -> Result<String, String> {
    let kind = target.target_kind.trim().to_ascii_lowercase();
    if !matches!(kind.as_str(), "ssh" | "wsl") {
        return Err("AOSP 仅支持 SSH 或 WSL Linux 目标。".to_string());
    }
    if target.target_id.trim().is_empty() || target.root.trim().is_empty() {
        return Err("请选择 Linux 目标并填写 AOSP 根目录。".to_string());
    }
    Ok(format!(
        "{}:{}:{}",
        kind,
        target.target_id.trim(),
        target.root.trim()
    ))
}

pub(super) fn run_aosp_linux_command(
    app: &AppHandle,
    target: &AospLinuxTargetRequest,
    command: &str,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<(i32, String, String), String> {
    run_aosp_linux_command_streaming(app, target, command, cancellation, timeout, None)
}

pub(super) fn run_aosp_linux_command_streaming(
    app: &AppHandle,
    target: &AospLinuxTargetRequest,
    command: &str,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
    sink: Option<agent_core::CommandOutputSink>,
) -> Result<(i32, String, String), String> {
    let kind = target.target_kind.trim().to_ascii_lowercase();
    let root = target.root.trim();
    match kind.as_str() {
        "wsl" => {
            let profile = crate::wsl::load_profile_for_agent(app, target.target_id.trim())?;
            let output = crate::wsl::run_wsl_workspace_command_streaming(
                &profile,
                root,
                command,
                &cancellation,
                timeout,
                sink,
            )?;
            Ok((
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ))
        }
        "ssh" => {
            let (config, password) =
                crate::ssh::load_connection_for_agent(app, target.target_id.trim())?;
            let output = agent_core::run_ssh_workspace_command_streaming(
                &config,
                password.as_deref(),
                root,
                command,
                cancellation,
                timeout,
                sink.as_ref(),
            )?;
            Ok((output.exit_code, output.stdout, output.stderr))
        }
        _ => Err("AOSP 仅支持 SSH 或 WSL Linux 目标。".to_string()),
    }
}

pub(super) fn bounded_output(stdout: String, stderr: String) -> String {
    const MAX: usize = 256 * 1024;
    let mut output = if stderr.trim().is_empty() {
        stdout
    } else {
        format!("{stdout}\n{stderr}")
    };
    if output.len() > MAX {
        output.truncate(MAX);
        output.push_str("\n[输出已截断]");
    }
    output
}

pub(super) fn clean_aosp_lunch_output(output: String) -> String {
    output
        .lines()
        .filter(|line| !line.contains("screen size is bogus"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// AOSP does not expose a reliable module-level build percentage. This only
/// supplies a conservative liveness value for a running task, never 100%.
pub(super) fn aosp_build_heartbeat_progress(elapsed_seconds: u64) -> (u8, u64) {
    const START: u8 = 18;
    const MAX: u8 = 92;
    const RAMP_SECONDS: u64 = 3 * 60 * 60;
    let elapsed_minutes = elapsed_seconds.div_ceil(60).max(1);
    let progress = (START as u64
        + elapsed_seconds.min(RAMP_SECONDS) * (MAX - START) as u64 / RAMP_SECONDS)
        .min(MAX as u64) as u8;
    (progress, elapsed_minutes)
}

pub(super) fn aosp_build_log_path(app: &AppHandle, task_id: &str) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位任务目录：{error}"))?
        .join("android-tasks")
        .join("aosp");
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建 AOSP 日志目录：{error}"))?;
    Ok(directory.join(format!("{task_id}.log")))
}

pub(super) fn parse_aosp_probe(target: &AospLinuxTargetRequest, output: &str) -> AospLinuxProbe {
    let field = |key: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    AospLinuxProbe {
        target_kind: target.target_kind.trim().to_ascii_lowercase(),
        target_id: target.target_id.trim().to_string(),
        root: target.root.trim().to_string(),
        linux: field("__AT_AOSP_LINUX__").as_deref() == Some("1"),
        bash_available: field("__AT_AOSP_BASH__").as_deref() == Some("1"),
        envsetup_path: field("__AT_AOSP_ENVSETUP__"),
        soong_path: field("__AT_AOSP_SOONG__"),
        manifest_path: field("__AT_AOSP_MANIFEST__"),
        android_bp_count: field("__AT_AOSP_BP_COUNT__").and_then(|value| value.parse().ok()),
        git_branch: field("__AT_AOSP_GIT_BRANCH__"),
        ccache_status: output
            .split_once("__AT_AOSP_CCACHE_BEGIN__\n")
            .and_then(|(_, tail)| tail.split_once("__AT_AOSP_CCACHE_END__"))
            .map(|(stats, _)| stats.trim().to_string())
            .filter(|stats| !stats.is_empty()),
        output: output
            .lines()
            .filter_map(|line| line.strip_prefix("__AT_AOSP_SIGNATURE__"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Verifies an AOSP tree in the selected SSH/WSL Linux filesystem. It only
/// reads metadata and deliberately does not touch the Windows filesystem.
#[tauri::command]
pub async fn probe_aosp_linux_workspace(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<AospLinuxProbe, String> {
    tauri::async_runtime::spawn_blocking(move || probe_aosp_linux_workspace_blocking(app, request))
        .await
        .map_err(|error| format!("AOSP 探测任务失败：{error}"))?
}

/// Finds likely AOSP roots on a configured Linux target. The command is
/// read-only, searches only common workspace mounts, and has both depth and
/// wall-clock limits so a large source checkout never blocks task planning.
#[tauri::command]
pub async fn discover_aosp_linux_workspaces(
    app: AppHandle,
    request: AospWorkspaceDiscoveryRequest,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let target = AospLinuxTargetRequest {
            target_kind: request.target_kind,
            target_id: request.target_id,
            root: "/".to_string(),
        };
        aosp_target_label(&target)?;
        let command = r#"timeout 10s sh -c 'for base in /home/*/workspace /home/*/src /data/*/workspace /data/*/src /workspace /mnt/*/workspace /opt /srv; do [ -d "$base" ] || continue; [ -f "$base/build/envsetup.sh" ] && printf "%s\n" "$base"; for root in "$base"/*; do [ -f "$root/build/envsetup.sh" ] && printf "%s\n" "$root"; done; done' | awk '!seen[$0]++' | head -n 20"#;
        let (code, stdout, stderr) = run_aosp_linux_command(
            &app,
            &target,
            command,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(30),
        )?;
        if code != 0 && stdout.trim().is_empty() {
            return Err(format!(
                "AOSP 目录检索失败（退出码 {code}）：{}",
                bounded_output(stdout, stderr)
            ));
        }
        let mut roots = stdout
            .lines()
            .map(str::trim)
            .filter(|root| root.starts_with('/') && !root.contains('\0'))
            .map(str::to_string)
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        roots.truncate(20);
        Ok(roots)
    })
    .await
    .map_err(|error| format!("AOSP 目录检索任务失败：{error}"))?
}

pub(super) const AOSP_PROBE_COMMAND: &str = r#"
printf '__AT_AOSP_LINUX__%s\n' "$(test "$(uname -s)" = Linux && echo 1 || echo 0)"
printf '__AT_AOSP_BASH__%s\n' "$(command -v bash >/dev/null 2>&1 && echo 1 || echo 0)"
test -f build/envsetup.sh && printf '__AT_AOSP_ENVSETUP__build/envsetup.sh\n'
test -f build/soong/soong_ui.bash && printf '__AT_AOSP_SOONG__build/soong/soong_ui.bash\n'
test -f .repo/manifest.xml && printf '__AT_AOSP_MANIFEST__.repo/manifest.xml\n'
count=$(timeout 20s bash -o pipefail -c 'find . \( -name .repo -o -name out -o -name .git \) -prune -o -name Android.bp -type f -printf "1\n" | wc -l' 2>/dev/null) && printf '__AT_AOSP_BP_COUNT__%s\n' "$count"
branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || git rev-parse --short HEAD 2>/dev/null || git -C .repo/manifests symbolic-ref --quiet --short HEAD 2>/dev/null || git -C .repo/manifests rev-parse --short HEAD 2>/dev/null)
test -n "$branch" && printf '__AT_AOSP_GIT_BRANCH__%s\n' "$branch"
stats=$(LC_ALL=C timeout 3s ccache -s 2>/dev/null) && printf '__AT_AOSP_CCACHE_BEGIN__\n%s\n__AT_AOSP_CCACHE_END__\n' "$stats"
kernel=$(uname -srmo 2>/dev/null); test -n "$kernel" && printf '__AT_AOSP_SIGNATURE__%s\n' "$kernel"
bash_version=$(bash --version 2>/dev/null | head -n 1); test -n "$bash_version" && printf '__AT_AOSP_SIGNATURE__%s\n' "$bash_version"
exit 0
"#;

pub(super) fn probe_aosp_linux_workspace_blocking(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<AospLinuxProbe, String> {
    aosp_target_label(&request)?;
    let command = AOSP_PROBE_COMMAND;
    let (code, stdout, stderr) = run_aosp_linux_command(
        &app,
        &request,
        command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(45),
    )?;
    if code != 0 {
        return Err(format!(
            "AOSP Linux 探测失败（退出码 {code}）：{}",
            bounded_output(stdout, stderr)
        ));
    }
    Ok(parse_aosp_probe(&request, &stdout))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospBuiltProduct {
    pub output: String,
    pub product: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospLunchCatalog {
    pub menu: String,
    pub built: Vec<AospBuiltProduct>,
}

#[tauri::command]
pub async fn list_aosp_lunch_targets(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<AospLunchCatalog, String> {
    tauri::async_runtime::spawn_blocking(move || list_aosp_lunch_targets_blocking(app, request))
        .await
        .map_err(|error| format!("lunch 目标读取任务失败：{error}"))?
}

pub(super) fn list_aosp_lunch_targets_blocking(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<AospLunchCatalog, String> {
    aosp_target_label(&request)?;
    let command = format!("test -f build/envsetup.sh || {{ echo '缺少 build/envsetup.sh' >&2; exit 2; }}; command -v bash >/dev/null || {{ echo '缺少 bash' >&2; exit 3; }}; bash -lc {}", aosp_shell_quote("source build/envsetup.sh >/dev/null && print_lunch_menu"));
    let (code, stdout, stderr) = run_aosp_linux_command(
        &app,
        &request,
        &command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(90),
    )?;
    if code != 0 {
        return Err(format!(
            "无法读取 lunch 目标（退出码 {code}）：{}",
            bounded_output(stdout, stderr)
        ));
    }
    let menu = clean_aosp_lunch_output(bounded_output(stdout, stderr));
    let built = list_packaged_aosp_products(&app, &request).unwrap_or_default();
    Ok(AospLunchCatalog { menu, built })
}

pub(super) fn list_packaged_aosp_products(
    app: &AppHandle,
    request: &AospLinuxTargetRequest,
) -> Result<Vec<AospBuiltProduct>, String> {
    // Product directory names often differ from the lunch combo. Read
    // previous_build_config.mk when it exists, and only mark a target packaged
    // when an image or zip is actually present. The search stops at the first hit.
    let command = r#"for base in out out_*; do
  [ -d "$base/target/product" ] || continue
  label=$(basename "$base")
  for dir in "$base"/target/product/*; do
    [ -d "$dir" ] || continue
    name=$(basename "$dir")
    case "$name" in obj|generic|common|symbols|.*) continue ;; esac
    if ! find "$dir" -maxdepth 3 \( -path '*/obj/*' -o -path '*/symbols/*' \) -prune -o -type f \( -name '*.img' -o -name '*.zip' \) -print 2>/dev/null | head -n 1 | grep -q .; then
      continue
    fi
    product="$name"
    variant=""
    mk="$dir/previous_build_config.mk"
    if [ -f "$mk" ]; then
      product=$(sed -n 's/^TARGET_PRODUCT[[:space:]]*:\{0,1\}=[[:space:]]*//p' "$mk" | head -n 1)
      variant=$(sed -n 's/^TARGET_BUILD_VARIANT[[:space:]]*:\{0,1\}=[[:space:]]*//p' "$mk" | head -n 1)
      product=${product:-$name}
    fi
    if [ -n "$variant" ]; then
      printf '%s\t%s-%s\n' "$label" "$product" "$variant"
    else
      printf '%s\t%s\n' "$label" "$product"
    fi
  done
done"#;
    let (code, stdout, _) = run_aosp_linux_command(
        app,
        request,
        command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(25),
    )?;
    if code != 0 {
        return Ok(Vec::new());
    }
    Ok(stdout
        .lines()
        .filter_map(|line| {
            let (output, product) = line.trim().split_once('\t')?;
            if output.is_empty() || product.is_empty() {
                return None;
            }
            Some(AospBuiltProduct {
                output: output.to_string(),
                product: product.to_string(),
            })
        })
        .take(80)
        .collect())
}

/// Lists a bounded set of installable or flashable artifacts generated under
/// the selected AOSP tree. The command is read-only and stays inside `out`.
#[tauri::command]
pub async fn list_aosp_directories(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        aosp_target_label(&request)?;
        let command = "printf '%s\\n' ..; find . -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%f\\n' 2>/dev/null | head -n 200";
        let (code, stdout, stderr) = run_aosp_linux_command(&app, &request, command, Arc::new(AtomicBool::new(false)), Duration::from_secs(20))?;
        if code != 0 { return Err(format!("无法读取目录：{}", bounded_output(stdout, stderr))); }
        Ok(stdout.lines().map(str::trim).filter(|line| !line.is_empty()).take(200).map(str::to_string).collect())
    })
    .await
    .map_err(|error| format!("目录读取任务失败：{error}"))?
}

#[tauri::command]
pub async fn list_aosp_build_artifacts(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<Vec<AospBuildArtifact>, String> {
    tauri::async_runtime::spawn_blocking(move || list_aosp_build_artifacts_blocking(app, request))
        .await
        .map_err(|error| format!("构建产物读取任务失败：{error}"))?
}

pub(super) fn list_aosp_build_artifacts_blocking(
    app: AppHandle,
    request: AospLinuxTargetRequest,
) -> Result<Vec<AospBuildArtifact>, String> {
    aosp_target_label(&request)?;
    // `obj` and system partitions can contain thousands of intermediate or
    // preinstalled packages. List standalone test outputs first, then append
    // final artifacts. Soong module APKs are only included from the explicit
    // AOSP sample root and only when they are the final signed package, never
    // as a broad escape hatch for all `.intermediates` content.
    let command = "{ find out/target/product -path '*/testcases/*' -type f \\( -name '*.apk' -o -name '*.apex' -o -name '*.img' -o -name '*.zip' -o -name '*.tar.gz' \\) -printf '%p\\t%s\\n' 2>/dev/null | head -n 80; find out/soong/.intermediates/development/samples -path '*/aapt2/*' -prune -o -type f -name '*.apk' ! -name '*-unsigned.apk' ! -name 'package-res.apk' -printf '%p\\t%s\\n' 2>/dev/null | head -n 80; find out/target/product -path '*/obj/*' -prune -o -path '*/testcases/*' -prune -o -maxdepth 8 -type f \\( -name '*.apk' -o -name '*.apex' -o -name '*.img' -o -name '*.zip' -o -name '*.tar.gz' \\) -printf '%p\\t%s\\n' 2>/dev/null | head -n 40; } | head -n 200";
    let (code, stdout, stderr) = run_aosp_linux_command(
        &app,
        &request,
        command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(45),
    )?;
    if code != 0 {
        return Err(format!(
            "无法读取 AOSP 构建产物（退出码 {code}）：{}",
            bounded_output(stdout, stderr)
        ));
    }
    Ok(stdout
        .lines()
        .filter_map(|line| {
            let (path, size) = line.rsplit_once('\t')?;
            let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
            let kind = match extension.as_str() {
                "apk" if path.starts_with("out/soong/.intermediates/development/samples/") => {
                    "Soong APK"
                }
                "apk" => "APK",
                "apex" => "APEX",
                "img" => "镜像",
                "zip" => "ZIP",
                "gz" => "压缩包",
                _ => "文件",
            }
            .to_string();
            let (installable, install_note) = aosp_apk_installability(path, &kind);
            Some(AospBuildArtifact {
                path: path.to_string(),
                size: size.parse().unwrap_or(0),
                kind,
                installable,
                install_note,
            })
        })
        .collect())
}

/// Product images contain preloaded and framework APKs. Keep them visible as
/// build outputs, but do not offer `adb install` for packages from a system
/// partition. The sole intermediate exception is an explicitly listed final
/// sample APK; it remains verified after staging before installation.
pub(super) fn aosp_apk_installability(path: &str, kind: &str) -> (bool, Option<String>) {
    if !matches!(kind, "APK" | "Soong APK") {
        return (false, Some("仅 APK 可以通过 ADB 安装。".to_string()));
    }
    let normalized = path.replace('\\', "/");
    if normalized.starts_with("out/soong/.intermediates/development/samples/") {
        let name = normalized.rsplit('/').next().unwrap_or_default();
        if name.ends_with("-unsigned.apk")
            || name == "package-res.apk"
            || normalized.contains("/aapt2/")
        {
            return (
                false,
                Some("这是 Soong 的未签名或资源中间包，不能直接安装。".to_string()),
            );
        }
        return (
            true,
            Some("来自 Soong 样例模块的最终 APK；安装前会暂存并进行设备端验证。".to_string()),
        );
    }
    let parts = normalized
        .split('/')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let output_start = parts
        .windows(3)
        .position(|window| window == ["out", "target", "product"])
        .map(|index| index + 4)
        .unwrap_or(parts.len()); // Skip output device name as well.
    let output_path = &parts[output_start..];
    let system_partition = output_path.iter().any(|part| {
        matches!(
            part.as_str(),
            "system"
                | "system_ext"
                | "product"
                | "vendor"
                | "odm"
                | "apex"
                | "priv-app"
                | "overlay"
        )
    });
    if system_partition {
        return (
            false,
            Some("这是系统分区预装包，需通过系统镜像或 adb root/remount 部署。".to_string()),
        );
    }
    if output_path.iter().any(|part| part == "obj") {
        return (
            false,
            Some("这是中间构建产物，请选择最终 APK 输出。".to_string()),
        );
    }
    (true, None)
}

pub(super) fn valid_aosp_token(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':' | '/')
        })
    {
        return Err(format!("{field} 格式无效。"));
    }
    Ok(value.to_string())
}

pub(super) fn checked_aosp_artifact_path(path: &str) -> Result<String, String> {
    let path = path.trim().replace('\\', "/");
    let supported_root = path.starts_with("out/target/product/")
        || path.starts_with("out/soong/.intermediates/development/samples/");
    if !supported_root
        || !path.to_ascii_lowercase().ends_with(".apk")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(
            "只能安装 AOSP out/target/product 下的最终 APK，或已列出的 Soong 样例 APK。"
                .to_string(),
        );
    }
    let kind = if path.starts_with("out/soong/.intermediates/development/samples/") {
        "Soong APK"
    } else {
        "APK"
    };
    let (installable, note) = aosp_apk_installability(&path, kind);
    if !installable {
        return Err(note.unwrap_or_else(|| "该 AOSP APK 不能作为普通应用安装。".to_string()));
    }
    Ok(path)
}

/// Stages an APK from the selected Linux AOSP tree and installs it through the
/// Windows ADB server. The staged APK remains as a task artifact for audit.
#[tauri::command]
pub fn start_aosp_artifact_install(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: AospArtifactInstallRequest,
) -> Result<String, String> {
    let target_label = aosp_target_label(&request.target)?;
    let serial = checked_serial(&request.serial)?.to_string();
    let artifact_path = checked_aosp_artifact_path(&request.artifact_path)?;
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "aosp-install",
        &format!("{target_label}:{serial}"),
        "正在准备 AOSP APK 安装...",
    )?;
    android_tasks::record_task_audit(
        &app,
        &task_id,
        "install-plan",
        &format!(
            "artifact={artifact_path}\nserial={serial}\nreplace={}",
            request.replace
        ),
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let stage_root = task_app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法定位 AOSP 暂存目录：{error}"))?
                .join("android-tasks")
                .join("aosp-artifacts")
                .join(&task_id_for_thread);
            fs::create_dir_all(&stage_root)
                .map_err(|error| format!("无法创建 AOSP 暂存目录：{error}"))?;
            let file_name = Path::new(&artifact_path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "AOSP APK 文件名无效。".to_string())?;
            let staged_apk = stage_root.join(file_name);
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                15,
                "正在从 Linux 工作区暂存 APK...",
            )?;
            match request
                .target
                .target_kind
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "wsl" => {
                    let profile = crate::wsl::load_profile_for_agent(
                        &task_app,
                        request.target.target_id.trim(),
                    )?;
                    let wsl_stage = crate::wsl::local_path_in_wsl(&profile, &staged_apk)?;
                    let command = format!(
                        "test -f {} && cp -- {} {}",
                        aosp_shell_quote(&artifact_path),
                        aosp_shell_quote(&artifact_path),
                        aosp_shell_quote(&wsl_stage)
                    );
                    let output = crate::wsl::run_wsl_workspace_command(
                        &profile,
                        &request.target.root,
                        &command,
                        &cancellation,
                        Duration::from_secs(10 * 60),
                    )?;
                    if !output.status.success() {
                        return Err(format!(
                            "无法暂存 WSL AOSP APK：{}",
                            bounded_output(
                                String::from_utf8_lossy(&output.stdout).into_owned(),
                                String::from_utf8_lossy(&output.stderr).into_owned()
                            )
                        ));
                    }
                }
                "ssh" => {
                    let (config, password) = crate::ssh::load_connection_for_agent(
                        &task_app,
                        request.target.target_id.trim(),
                    )?;
                    agent_core::download_ssh_workspace_file(
                        &config,
                        password.as_deref(),
                        &request.target.root,
                        &artifact_path,
                        &staged_apk,
                        cancellation.clone(),
                        Duration::from_secs(10 * 60),
                    )?;
                }
                _ => return Err("AOSP 仅支持 SSH 或 WSL Linux 目标。".to_string()),
            }
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            let metadata = fs::metadata(&staged_apk)
                .map_err(|error| format!("无法读取已暂存 APK：{error}"))?;
            if metadata.len() == 0 {
                return Err("AOSP APK 暂存结果为空。".to_string());
            }
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "staged",
                &format!("path={}\nsize={}", staged_apk.display(), metadata.len()),
            )?;
            let staged_sha256 = sha256(&staged_apk)?;
            let package_name = apk_package_name(&staged_apk)?;
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "package",
                &format!("aapt2 package={package_name}\nsha256={staged_sha256}"),
            )?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                75,
                "正在通过 Windows ADB 安装 APK...",
            )?;
            android_tasks::enter_non_cancellable_phase(
                &task_app,
                &manager,
                &task_id_for_thread,
                "已进入 Windows ADB 设备安装阶段；后续将记录实际设备结果。",
            )?;
            let allow_test = artifact_path.split('/').any(|part| part == "testcases")
                || artifact_path.starts_with("out/soong/.intermediates/development/samples/");
            let install = install_adb_package_with_audit(
                task_app.clone(),
                AdbPackageInstallRequest {
                    serial: serial.clone(),
                    apk_path: staged_apk.to_string_lossy().to_string(),
                    replace: request.replace,
                    allow_test,
                },
                Some(&task_id_for_thread),
            )?;
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "install-result",
                &install,
            )?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                88,
                "正在验证设备端已安装包...",
            )?;
            let adb = find_adb()?;
            let verify_args = [
                "-s",
                serial.as_str(),
                "shell",
                "pm",
                "path",
                package_name.as_str(),
            ];
            let mut verify_command = Command::new(&adb);
            verify_command.args(verify_args);
            let verify = command_output_with_timeout(verify_command, Duration::from_secs(30))
                .map_err(|error| format!("无法验证设备端 APK：{error}"))?;
            android_tasks::record_command_audit(
                &task_app,
                &task_id_for_thread,
                "验证 AOSP APK 已安装",
                &adb.to_string_lossy(),
                &verify_args,
                &verify,
            )?;
            if !verify.status.success()
                || !String::from_utf8_lossy(&verify.stdout)
                    .lines()
                    .any(|line| line.trim_start().starts_with("package:"))
            {
                return Err(format!("ADB 安装后未找到包 {package_name}。"));
            }
            fs::write(stage_root.join("install-result.txt"), &install)
                .map_err(|error| format!("无法保存安装结果：{error}"))?;
            fs::write(stage_root.join("metadata.txt"), format!(
                "task_id={task_id_for_thread}\nsource_target={target_label}\nsource_root={}\nsource_artifact={artifact_path}\nstaged_apk={}\nstaged_size={}\nstaged_sha256={staged_sha256}\npackage={package_name}\nserial={serial}\nreplace={}\nallow_test={allow_test}\npm_path={}\n",
                request.target.root,
                staged_apk.display(),
                metadata.len(),
                request.replace,
                String::from_utf8_lossy(&verify.stdout).trim(),
            )).map_err(|error| format!("无法保存 AOSP APK 安装元数据：{error}"))?;
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                96,
                "正在保存 AOSP APK 安装结果...",
            )?;
            Ok(stage_root)
        })();
        let artifact = result.as_ref().ok().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

/// Starts an explicit AOSP module build in the chosen Linux workspace. The
/// full output is saved under the app task directory for later diagnostics.
#[tauri::command]
pub fn start_aosp_linux_build(
    app: AppHandle,
    state: State<'_, AndroidTaskManager>,
    request: AospLinuxBuildRequest,
) -> Result<String, String> {
    let target_label = aosp_target_label(&request.target)?;
    let custom = request.custom_command.trim().to_string();
    if custom.contains('\0') || custom.len() > 64 * 1024 {
        return Err("自定义命令含有无效字符或超过 64 KiB。".to_string());
    }
    let lunch = if custom.is_empty() {
        valid_aosp_token(&request.lunch_target, "Lunch 目标")?
    } else {
        String::new()
    };
    let module = if !custom.is_empty() || request.module.trim().is_empty() {
        String::new()
    } else {
        valid_aosp_token(&request.module, "构建模块")?
    };
    let manager = state.inner().clone();
    let (task_id, cancellation) = android_tasks::create_task(
        &app,
        &manager,
        "aosp-build",
        &target_label,
        "正在准备 Linux AOSP 构建...",
    )?;
    let build_command = if !custom.is_empty() {
        custom.clone()
    } else if module.is_empty() {
        "m".to_string()
    } else {
        format!("m {module}")
    };
    android_tasks::record_task_audit(
        &app,
        &task_id,
        "build-plan",
        &format!(
            "Linux target={target_label}\nroot={}\nlunch={lunch}\nmodule={build_command}",
            request.target.root
        ),
    )?;
    let task_app = app.clone();
    let task_id_for_thread = task_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let log_path = aosp_build_log_path(&task_app, &task_id_for_thread)?;
            let mut file =
                File::create(&log_path).map_err(|error| format!("无法创建构建日志：{error}"))?;
            writeln!(file, "target={target_label}\nroot={}\nlunch={lunch}\nmodule={module}\ncommand={build_command}\n", request.target.root)
                .map_err(|error| format!("无法保存构建日志：{error}"))?;
            let file = Arc::new(Mutex::new(file));
            let output_file = file.clone();
            let log_error = Arc::new(Mutex::new(None::<String>));
            let output_error = log_error.clone();
            let sink = agent_core::CommandOutputSink::new(
                Arc::new(move |event| {
                    if let Some(text) = event.get("text").and_then(serde_json::Value::as_str) {
                        if let Ok(mut file) = output_file.lock() {
                            if let Err(error) = file.write_all(text.as_bytes()) {
                                if let Ok(mut failed) = output_error.lock() {
                                    *failed = Some(error.to_string());
                                }
                            }
                        }
                    }
                }),
                &task_id_for_thread,
                "",
                "aosp-build",
            )
            .continuous();
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                8,
                "正在检查 SSH/WSL AOSP 工作区...",
            )?;
            let probe_command = if custom.is_empty() {
                "test -f build/envsetup.sh && test -f build/soong/soong_ui.bash && command -v bash >/dev/null"
            } else {
                "test -d . && command -v bash >/dev/null"
            };
            let (probe_code, probe_stdout, probe_stderr) = run_aosp_linux_command(
                &task_app,
                &request.target,
                probe_command,
                cancellation.clone(),
                Duration::from_secs(45),
            )?;
            if probe_code != 0 {
                return Err(format!(
                    "所选 Linux 工作目录或构建环境不可用：{}",
                    bounded_output(probe_stdout, probe_stderr)
                ));
            }
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                18,
                if custom.is_empty() {
                    "正在初始化 AOSP 环境并执行 lunch..."
                } else {
                    "正在执行自定义命令..."
                },
            )?;
            let build = if module.is_empty() {
                "m".to_string()
            } else {
                format!("m {}", aosp_shell_quote(&module))
            };
            let script = if !custom.is_empty() {
                format!("set -o pipefail; export PYTHONUNBUFFERED=1;\n{custom}")
            } else {
                format!(
                "set -o pipefail; export PYTHONUNBUFFERED=1; source build/envsetup.sh && lunch {} && {}",
                aosp_shell_quote(&lunch),
                build
            )
            };
            let command = format!("bash -lc {}", aosp_shell_quote(&script));
            let (build_done, build_finished) = mpsc::channel::<()>();
            let heartbeat_app = task_app.clone();
            let heartbeat_task_id = task_id_for_thread.clone();
            let heartbeat_cancellation = cancellation.clone();
            let heartbeat = std::thread::spawn(move || {
                let started = std::time::Instant::now();
                loop {
                    match build_finished.recv_timeout(Duration::from_secs(1)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout)
                            if heartbeat_cancellation.load(Ordering::Relaxed) =>
                        {
                            break
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let (progress, _) =
                                aosp_build_heartbeat_progress(started.elapsed().as_secs());
                            let _ = android_tasks::update_task(
                                &heartbeat_app,
                                &heartbeat_task_id,
                                progress,
                                &format!("命令运行中 · 已运行 {} 秒", started.elapsed().as_secs()),
                            );
                        }
                    }
                }
            });
            // Always stop the liveness worker before propagating a transport
            // failure. Otherwise an SSH/WSL error could leave a detached
            // heartbeat updating a task that has already been marked failed.
            let build_output = run_aosp_linux_command_streaming(
                &task_app,
                &request.target,
                &command,
                cancellation.clone(),
                Duration::from_secs(60 * 60 * 4),
                Some(sink),
            );
            let _ = build_done.send(());
            let _ = heartbeat.join();
            let (code, _, _) = match build_output {
                Ok(output) => output,
                Err(error) => {
                    writeln!(
                        file.lock().map_err(|_| "构建日志锁不可用")?,
                        "\ntransport_error={error}"
                    )
                    .map_err(|write_error| format!("无法保存 AOSP 构建错误日志：{write_error}"))?;
                    android_tasks::record_task_audit(
                        &task_app,
                        &task_id_for_thread,
                        "transport-error",
                        &format!("log={}\n{error}", log_path.display()),
                    )?;
                    return Err(error);
                }
            };
            if let Some(error) = log_error.lock().map_err(|_| "构建日志锁不可用")?.as_ref()
            {
                return Err(format!("构建日志写入失败：{error}"));
            }
            writeln!(
                file.lock().map_err(|_| "构建日志锁不可用")?,
                "\nexit_code={code}"
            )
            .map_err(|error| format!("无法保存构建日志：{error}"))?;
            android_tasks::record_task_audit(
                &task_app,
                &task_id_for_thread,
                "command-result",
                &format!("exit_code={code}\nlog={}", log_path.display()),
            )?;
            if cancellation.load(Ordering::Relaxed) {
                return Err("请求已取消".to_string());
            }
            if code != 0 {
                return Err(format!(
                    "AOSP 构建失败（退出码 {code}），请查看任务产物日志。"
                ));
            }
            android_tasks::update_task(
                &task_app,
                &task_id_for_thread,
                96,
                "正在保存 AOSP 构建日志...",
            )?;
            Ok(log_path)
        })();
        let failed_log = aosp_build_log_path(&task_app, &task_id_for_thread)
            .ok()
            .filter(|path| path.is_file());
        let artifact_path = result.as_ref().ok().cloned().or(failed_log);
        let artifact = artifact_path.as_deref().and_then(|path| path.to_str());
        let error = result.as_ref().err().map(String::as_str);
        let _ =
            android_tasks::finish_task(&task_app, &manager, &task_id_for_thread, artifact, error);
    });
    Ok(task_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AospLiveOutput {
    pub(super) text: String,
    pub(super) next_offset: u64,
}

/// Read only the newly written portion of a managed log, including while its
/// task is running. Byte offsets avoid losing output across polling intervals.
#[tauri::command]
pub fn read_aosp_live_output(
    app: AppHandle,
    task_id: String,
    offset: u64,
) -> Result<AospLiveOutput, String> {
    let connection = android_tasks::task_store(&app)?;
    connection
        .query_row(
            "SELECT id FROM tasks WHERE id=?1 AND kind='aosp-build'",
            [&task_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| "未找到 AOSP 构建任务。".to_string())?;
    if !task_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err("构建任务 ID 无效。".to_string());
    }
    let path = aosp_build_log_path(&app, &task_id)?;
    read_aosp_output_chunk(&path, offset)
}

pub(super) fn read_aosp_output_chunk(path: &Path, offset: u64) -> Result<AospLiveOutput, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AospLiveOutput {
                text: String::new(),
                next_offset: offset,
            })
        }
        Err(error) => return Err(format!("无法读取构建日志：{error}")),
    };
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(64 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    // Do not split a UTF-8 code point at the chunk boundary.
    let length = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => bytes.len(),
    };
    Ok(AospLiveOutput {
        text: String::from_utf8_lossy(&bytes[..length]).into_owned(),
        next_offset: offset + length as u64,
    })
}

pub fn analyze_aosp_build_log(path: String) -> Result<AospBuildLogAnalysis, String> {
    let path =
        fs::canonicalize(path.trim()).map_err(|error| format!("无法读取构建日志：{error}"))?;
    let bytes = fs::read(&path).map_err(|error| format!("无法读取构建日志：{error}"))?;
    let truncated = bytes.len() > 8 * 1024 * 1024;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8 * 1024 * 1024)]);
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if (lower.contains("error:")
            || lower.starts_with("transport_error=")
            || lower.contains("failed")
            || lower.starts_with("ninja: error"))
            && errors.len() < 200
        {
            errors.push(line.to_string());
        } else if lower.contains("warning:") && warnings.len() < 100 {
            warnings.push(line.to_string());
        }
    }
    Ok(AospBuildLogAnalysis {
        path: path.to_string_lossy().to_string(),
        errors,
        warnings,
        truncated,
    })
}

/// Reads an AOSP build log only through its completed managed task record.
/// The WebView never supplies a filesystem path for this operation.
#[tauri::command]
pub fn read_aosp_build_log(
    app: AppHandle,
    task_id: String,
) -> Result<AospBuildLogAnalysis, String> {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("任务 ID 不能为空。".to_string());
    }
    let connection = android_tasks::task_store(&app)?;
    let artifact: String = connection.query_row(
        "SELECT artifact_path FROM tasks WHERE id=?1 AND kind='aosp-build' AND status IN ('done','error') AND artifact_path IS NOT NULL",
        [task_id],
        |row| row.get(0),
    ).map_err(|_| "未找到包含受管日志的 AOSP 构建任务。".to_string())?;
    let path =
        fs::canonicalize(&artifact).map_err(|error| format!("无法读取 AOSP 构建日志：{error}"))?;
    let root = fs::canonicalize(
        app.path()
            .app_data_dir()
            .map_err(|error| format!("无法定位任务目录：{error}"))?
            .join("android-tasks")
            .join("aosp"),
    )
    .map_err(|error| format!("无法定位 AOSP 任务目录：{error}"))?;
    let expected_name = format!("{task_id}.log");
    if !path.starts_with(&root)
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
    {
        return Err("AOSP 构建日志路径无效。".to_string());
    }
    analyze_aosp_build_log(path.to_string_lossy().to_string())
}

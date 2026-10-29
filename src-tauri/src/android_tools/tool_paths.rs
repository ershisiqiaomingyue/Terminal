use super::*;

pub(crate) fn find_adb() -> Result<PathBuf, String> {
    if let Some(path) = configured_environment_tool_path("adb") {
        return Ok(path);
    }
    let output = if cfg!(target_os = "windows") {
        background_command("where.exe").arg("adb.exe").output()
    } else {
        background_command("which").arg("adb").output()
    }
    .map_err(|error| format!("无法查找 adb：{error}"))?;

    if output.status.success() {
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
        {
            return Ok(PathBuf::from(path));
        }
    }
    let mut sdk_roots = Vec::new();
    if let Some(home) =
        std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
    {
        sdk_roots.push(PathBuf::from(home));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        sdk_roots.push(PathBuf::from(local_app_data).join("Android").join("Sdk"));
    }
    for root in sdk_roots {
        let candidate = root
            .join("platform-tools")
            .join(if cfg!(target_os = "windows") {
                "adb.exe"
            } else {
                "adb"
            });
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("未找到 adb。请安装 Android SDK Platform-Tools，并将其目录加入 PATH。".to_string())
}

pub(super) fn find_aapt2() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("aapt2") {
        return Some(path);
    }
    let executable = if cfg!(target_os = "windows") {
        "aapt2.exe"
    } else {
        "aapt2"
    };
    let on_path = if cfg!(target_os = "windows") {
        background_command("where.exe")
            .arg(executable)
            .output()
            .ok()
    } else {
        background_command("which").arg(executable).output().ok()
    };
    if let Some(output) = on_path.filter(|output| output.status.success()) {
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
        {
            return Some(PathBuf::from(path));
        }
    }

    let mut roots = Vec::new();
    if let Some(home) =
        std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
    {
        roots.push(PathBuf::from(home));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local_app_data).join("Android").join("Sdk"));
    }
    for root in roots {
        let build_tools = root.join("build-tools");
        let Ok(read_dir) = fs::read_dir(build_tools) else {
            continue;
        };
        let mut versions = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
        versions.sort_by_key(|entry| entry.file_name());
        for version in versions.into_iter().rev() {
            let candidate = version.path().join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub(super) fn apk_package_name(apk: &Path) -> Result<String, String> {
    let aapt2 =
        find_aapt2().ok_or_else(|| "未找到 aapt2，无法验证已安装 APK 的包名。".to_string())?;
    let mut command = Command::new(&aapt2);
    command.args(["dump", "packagename"]).arg(apk);
    let output =
        command_output_with_timeout_limit(command, Duration::from_secs(30), MAX_AAPT_OUTPUT_BYTES)
            .map_err(|error| format!("无法读取 APK 包名：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法读取 APK 包名：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let package = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    checked_package(&package)?;
    Ok(package)
}

pub(super) fn apk_min_sdk(apk: &Path) -> Result<Option<String>, String> {
    let aapt2 = find_aapt2().ok_or_else(|| "未找到 aapt2，无法检查 APK 平台要求。".to_string())?;
    let mut command = Command::new(&aapt2);
    command.args(["dump", "badging"]).arg(apk);
    let output =
        command_output_with_timeout_limit(command, Duration::from_secs(30), MAX_AAPT_OUTPUT_BYTES)
            .map_err(|error| format!("无法读取 APK 平台要求：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法读取 APK 平台要求：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("minSdkVersion:")
                .map(|value| value.trim().trim_matches('\'').to_string())
                .filter(|value| !value.is_empty())
        }))
}

/// Returns an actionable incompatibility only when the APK and device report
/// enough information to prove it. Unknown future codenames still go through
/// to ADB, which remains the final compatibility authority.
pub(super) fn apk_device_compatibility(
    min_sdk: Option<&str>,
    device_sdk: Option<u32>,
    device_codename: &str,
) -> Option<String> {
    let min_sdk = min_sdk?.trim();
    if min_sdk.is_empty() {
        return None;
    }
    if let Ok(required) = min_sdk.parse::<u32>() {
        if device_sdk.is_some_and(|actual| actual < required) {
            return Some(format!(
                "APK 最低 SDK 为 {required}，设备 SDK 为 {}。",
                device_sdk.unwrap_or_default()
            ));
        }
        return None;
    }
    if device_codename.trim().eq_ignore_ascii_case("REL") {
        return Some(format!(
            "APK 要求开发版 Android 平台 {min_sdk}，当前设备是 release 镜像。"
        ));
    }
    None
}

pub(super) fn check_apk_device_compatibility(serial: &str, apk: &Path) -> Result<(), String> {
    let min_sdk = apk_min_sdk(apk)?;
    let sdk = adb_output(&["-s", serial, "shell", "getprop", "ro.build.version.sdk"])
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let codename = adb_output(&[
        "-s",
        serial,
        "shell",
        "getprop",
        "ro.build.version.codename",
    ])
    .unwrap_or_default();
    if let Some(reason) = apk_device_compatibility(min_sdk.as_deref(), sdk, &codename) {
        return Err(format!("安装前兼容性检查失败：{reason}"));
    }
    Ok(())
}

pub(super) fn find_android_build_tool(executable: &str) -> Option<PathBuf> {
    if executable.eq_ignore_ascii_case(if cfg!(target_os = "windows") {
        "aapt2.exe"
    } else {
        "aapt2"
    }) {
        if let Some(path) = configured_environment_tool_path("aapt2") {
            return Some(path);
        }
    }
    if let Some(path) = configured_environment_tool_path("aapt2")
        .and_then(|path| path.parent().map(|parent| parent.join(executable)))
        .filter(|path| path.is_file())
        .or_else(|| crate::bundled_environment::build_tool_path(executable))
    {
        return Some(path);
    }
    let on_path = if cfg!(target_os = "windows") {
        background_command("where.exe")
            .arg(executable)
            .output()
            .ok()
    } else {
        background_command("which").arg(executable).output().ok()
    };
    if let Some(output) = on_path.filter(|output| output.status.success()) {
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
        {
            return Some(PathBuf::from(path));
        }
    }
    let mut roots = Vec::new();
    if let Some(home) =
        std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
    {
        roots.push(PathBuf::from(home));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local_app_data).join("Android").join("Sdk"));
    }
    for root in roots {
        let Ok(read_dir) = fs::read_dir(root.join("build-tools")) else {
            continue;
        };
        let mut versions = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
        versions.sort_by_key(|entry| entry.file_name());
        for version in versions.into_iter().rev() {
            let candidate = version.path().join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub(super) fn find_java() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("java") {
        return Some(path);
    }
    if let Some(java_home) = std::env::var_os("JAVA_HOME") {
        let executable = if cfg!(target_os = "windows") {
            "java.exe"
        } else {
            "java"
        };
        let candidate = PathBuf::from(java_home).join("bin").join(executable);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let executable = if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    };
    let studio_java = program_files
        .join("Android")
        .join("Android Studio")
        .join("jbr")
        .join("bin")
        .join(executable);
    if studio_java.is_file() {
        return Some(studio_java);
    }
    let output = if cfg!(target_os = "windows") {
        background_command("where.exe")
            .arg(executable)
            .output()
            .ok()
    } else {
        background_command("which").arg(executable).output().ok()
    }?;
    output
        .status
        .success()
        .then(|| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(PathBuf::from)
        })
        .flatten()
}

/// Locate the optional system Gradle executable. Project Gradle wrappers are
/// intentionally not configured here: `run_project_check` resolves them from
/// the selected project directory at execution time.
pub(super) fn find_gradle() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("gradle") {
        return Some(path);
    }
    let candidates: &[&str] = if cfg!(target_os = "windows") {
        &["gradle", "gradle.bat", "gradle.cmd", "gradle.exe"]
    } else {
        &["gradle"]
    };
    let resolver = if cfg!(target_os = "windows") {
        "where.exe"
    } else {
        "which"
    };
    candidates.iter().find_map(|executable| {
        let output = background_command(resolver).arg(executable).output().ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
            .map(PathBuf::from)
    })
}

pub(super) fn find_apktool() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("apktool") {
        return Some(path);
    }
    if let Some(path) = std::env::var_os("ANDROID_TOOL_APKTOOL_JAR")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let tools = environment_tools_root().ok()?;
    fs::read_dir(tools)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("apktool_") && name.ends_with(".jar"))
        })
}

pub(super) fn find_jadx() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("jadx") {
        return Some(path);
    }
    if let Some(path) = std::env::var_os("ANDROID_TOOL_JADX")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let executable = if cfg!(target_os = "windows") {
        "jadx.bat"
    } else {
        "jadx"
    };
    let on_path = if cfg!(target_os = "windows") {
        background_command("where.exe")
            .arg(executable)
            .output()
            .ok()
    } else {
        background_command("which").arg(executable).output().ok()
    };
    if let Some(output) = on_path.filter(|output| output.status.success()) {
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
        {
            return Some(PathBuf::from(path));
        }
    }
    let root = environment_tools_root().ok()?;
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("bin").join(executable))
        .find(|path| path.is_file())
}
pub(super) fn find_bundletool() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("bundletool") {
        return Some(path);
    }
    if let Some(path) = std::env::var_os("ANDROID_TOOL_BUNDLETOOL_JAR")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    fs::read_dir(environment_tools_root().ok()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("bundletool-all-") && name.ends_with(".jar"))
        })
}
pub(super) fn find_trace_processor() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ANDROID_TOOL_TRACE_PROCESSOR")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let executable = if cfg!(target_os = "windows") {
        "trace_processor_shell.exe"
    } else {
        "trace_processor_shell"
    };
    fs::read_dir(environment_tools_root().ok()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find_map(|root| {
            let direct = root.join(executable);
            if direct.is_file() {
                return Some(direct);
            }
            fs::read_dir(&root)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(executable))
                .find(|path| path.is_file())
        })
}
pub(super) fn find_hprof_converter() -> Option<PathBuf> {
    let executable = if cfg!(target_os = "windows") {
        "hprof-conv.exe"
    } else {
        "hprof-conv"
    };
    if let Some(path) = std::env::var_os("ANDROID_TOOL_HPROF_CONV")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    // hprof-conv ships inside the bundled platform-tools directory, so prefer it
    // before falling back to whatever Android SDK the machine happens to have.
    if let Some(bundled) = configured_environment_tool_path("adb")
        .and_then(|adb| adb.parent().map(|parent| parent.join(executable)))
        .filter(|path| path.is_file())
    {
        return Some(bundled);
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let sdk = std::env::var_os("ANDROID_HOME")
        .or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
        .map(PathBuf::from)
        .unwrap_or_else(|| local.join("Android").join("Sdk"));
    let candidate = sdk.join("platform-tools").join(executable);
    candidate.is_file().then_some(candidate)
}
pub(super) fn find_shark_runtime(
    app: &AppHandle,
) -> Result<(PathBuf, PathBuf, Vec<PathBuf>), String> {
    let java =
        find_java().ok_or_else(|| "未找到 Java 运行时，无法运行 Shark 对象图分析。".to_string())?;
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("无法定位 Shark 资源：{error}"))?;
    let resource_root = [
        resource_dir.join("shark"),
        resource_dir.join("resources").join("shark"),
    ]
    .into_iter()
    .find(|path| path.is_dir())
    .ok_or_else(|| "未找到 Shark 资源目录。请重新安装 Android Toolkit。".to_string())?;
    let helper = resource_root.join("android-tool-shark.jar");
    if !helper.is_file() {
        return Err("未找到 Shark 分析辅助程序。请重新安装 Android Toolkit。".to_string());
    }
    let studio = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
        .join("Android")
        .join("Android Studio");
    let android_lib = studio.join("plugins").join("android").join("lib");
    let kotlin_lib = studio
        .join("plugins")
        .join("Kotlin")
        .join("kotlinc")
        .join("lib");
    let dependencies = [
        android_lib.join("shark-2.14.jar"),
        android_lib.join("shark-android-2.14.jar"),
        android_lib.join("shark-graph-2.14.jar"),
        android_lib.join("shark-hprof-2.14.jar"),
        android_lib.join("shark-log-2.14.jar"),
        kotlin_lib.join("kotlin-stdlib.jar"),
        android_lib.join("okio-jvm-3.9.0.jar"),
    ];
    if dependencies.iter().any(|path| !path.is_file()) {
        return Err(
            "未找到 Android Studio 自带 Shark 2.14 运行依赖。当前安装无法执行对象图分析。"
                .to_string(),
        );
    }
    Ok((java, helper, dependencies.into_iter().collect()))
}
pub(super) fn find_ndk_stack() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ANDROID_TOOL_NDK_STACK")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let name = if cfg!(target_os = "windows") {
        "ndk-stack.cmd"
    } else {
        "ndk-stack"
    };
    fs::read_dir(local.join("Android").join("Sdk").join("ndk"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("ndk-stack.cmd"))
        .find(|path| path.is_file())
        .or_else(|| {
            fs::read_dir(local.join("Android").join("Sdk").join("ndk"))
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| {
                    entry
                        .path()
                        .join("prebuilt")
                        .join("windows-x86_64")
                        .join("bin")
                        .join(name)
                })
                .find(|path| path.is_file())
        })
}
pub(super) fn find_llvm_addr2line() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ANDROID_TOOL_LLVM_ADDR2LINE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let executable = if cfg!(target_os = "windows") {
        "llvm-addr2line.exe"
    } else {
        "llvm-addr2line"
    };
    fs::read_dir(local.join("Android").join("Sdk").join("ndk"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| {
            entry
                .path()
                .join("toolchains")
                .join("llvm")
                .join("prebuilt")
                .join(if cfg!(target_os = "windows") {
                    "windows-x86_64"
                } else {
                    "linux-x86_64"
                })
                .join("bin")
                .join(executable)
        })
        .find(|path| path.is_file())
}
pub(super) fn find_scrcpy() -> Option<PathBuf> {
    if let Some(path) = configured_environment_tool_path("scrcpy") {
        return Some(path);
    }
    if let Some(path) = std::env::var_os("ANDROID_TOOL_SCRCPY")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let name = if cfg!(target_os = "windows") {
        "scrcpy.exe"
    } else {
        "scrcpy"
    };
    let on_path = if cfg!(target_os = "windows") {
        background_command("where.exe").arg(name).output().ok()
    } else {
        background_command("which").arg(name).output().ok()
    };
    if let Some(output) = on_path.filter(|output| output.status.success()) {
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
        {
            return Some(PathBuf::from(path));
        }
    }
    let mut directories = vec![(environment_tools_root().ok()?, 0usize)];
    while let Some((directory, depth)) = directories.pop() {
        let direct = directory.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        if depth >= 2 {
            continue;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        directories.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| (entry.path(), depth + 1))
        }));
    }
    None
}

pub(super) fn environment_tool_command(program: &Path) -> Command {
    let is_batch = cfg!(target_os = "windows")
        && program.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
        });
    // Rust wraps .bat/.cmd in cmd.exe and escapes batch arguments at spawn time.
    // An explicit cmd.exe wrapper would bypass that batch-specific escaping.
    let mut command = Command::new(normalized_environment_tool_path(program));
    for (name, value) in agent_core::child_proxy_environment() {
        command.env(name, value);
    }
    // Batch launchers (JADX/apksigner) must see the selected Java runtime,
    // even on a clean Windows machine with no global JAVA_HOME or PATH entry.
    if let Some(java) = is_batch.then(find_java).flatten() {
        if let Some(bin) = java.parent() {
            if let Some(home) = bin.parent() {
                command.env("JAVA_HOME", normalized_environment_tool_path(home));
                let mut paths = vec![normalized_environment_tool_path(bin)];
                if let Some(existing) = std::env::var_os("PATH") {
                    paths.extend(std::env::split_paths(&existing));
                }
                if let Ok(path) = std::env::join_paths(paths) {
                    command.env("PATH", path);
                }
            }
        }
    }
    command
}

pub(super) fn command_output(
    program: &Path,
    args: &[String],
) -> Result<std::process::Output, String> {
    let mut command = environment_tool_command(program);
    command.args(args);
    command_output_with_timeout(command, Duration::from_secs(10 * 60))
        .map_err(|error| format!("无法运行 {}：{error}", program.display()))
}

pub(super) fn require_success(
    program: &Path,
    args: &[String],
    action: &str,
) -> Result<String, String> {
    let output = command_output(program, args)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        return Ok(if stdout.is_empty() { stderr } else { stdout });
    }
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(format!(
        "{action}失败：{}",
        if detail.is_empty() {
            "工具没有返回错误信息"
        } else {
            &detail
        }
    ))
}

pub(super) fn debug_keystore(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?
        .join("keystores");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建密钥目录：{error}"))?;
    let keystore = root.join("debug.keystore");
    if keystore.is_file() {
        return Ok(keystore);
    }
    let java = find_java().ok_or_else(|| "未找到 Java，无法创建调试签名密钥。".to_string())?;
    let keytool = java
        .parent()
        .map(|parent| {
            parent.join(if cfg!(target_os = "windows") {
                "keytool.exe"
            } else {
                "keytool"
            })
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| "当前 Java 运行时不包含 keytool。".to_string())?;
    let args = [
        "-genkeypair",
        "-keystore",
        keystore.to_string_lossy().as_ref(),
        "-storepass",
        "android",
        "-keypass",
        "android",
        "-alias",
        "androiddebugkey",
        "-dname",
        "CN=Android Debug,O=Android,C=US",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-validity",
        "10000",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    require_success(&keytool, &args, "创建调试签名密钥")?;
    Ok(keystore)
}

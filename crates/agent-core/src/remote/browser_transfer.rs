use super::browser_transfer_transport::{append_binary, transfer_binary};
use super::*;
use crate::transfer_archive::{
    local_transfer_size, pack_transfer_archive, safe_transfer_name, unpack_transfer_archive,
};

pub type TransferProgress = Arc<dyn Fn(&str, u64, u64) + Send + Sync>;
pub const LARGE_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTransferInfo {
    pub name: String,
    pub bytes: u64,
    pub requires_confirmation: bool,
    /// 远端是不是目录。目录走远程 tar 打包，单个文件走可续传的裸流。
    pub is_directory: bool,
    /// 远端文件的版本标记（设备号:inode:大小:修改时间），用于判断断点是否还属于
    /// 同一份内容 —— 文件被重新编译覆盖过就必须从头下，不能接着旧断点。
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadConflict {
    pub name: String,
    pub version: String,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadPolicy {
    Reject,
    Overwrite,
    Rename,
}

pub fn transfer_connection(config: &SshConnectionConfig, suffix: &str) -> SshConnectionConfig {
    // 真标记判定：russh 引擎可用时不粘 ssh.exe，泳道键在引擎侧归一（S3）。
    let fallback = is_openssh_transport_marked(config);
    let mut connection = config.clone();
    connection.id.push_str(suffix);
    if fallback {
        remember_openssh_transport(&connection);
    }
    connection
}

fn validate_destination(path: &str) -> Result<(), String> {
    if !path.starts_with('/') || path.contains('\0') || path.split('/').any(|part| part == "..") {
        return Err("请选择服务器绝对目录".into());
    }
    Ok(())
}
fn upload_names(paths: &[String]) -> Result<Vec<String>, String> {
    let mut names = HashSet::new();
    paths
        .iter()
        .map(|path| {
            let name = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("上传文件名无效")?
                .to_string();
            if !safe_transfer_name(&name) || !names.insert(name.clone()) {
                return Err("上传文件名无效或所选文件有重复名称".into());
            }
            Ok(name)
        })
        .collect()
}
pub fn inspect_ssh_browser_upload(
    config: &SshConnectionConfig,
    password: Option<&str>,
    paths: &[String],
    destination: &str,
) -> Result<Vec<UploadConflict>, String> {
    validate_destination(destination)?;
    let names = upload_names(paths)?;
    let commands = names.iter().enumerate().map(|(index, name)| {
        let path = shell_escape(&format!("{}/{name}", destination.trim_end_matches('/')));
        format!("if [ -e {path} ] || [ -L {path} ]; then printf '{index} '; stat -c '%d:%i:%s:%y:%z' -- {path} || exit 1; fi;")
    }).collect::<Vec<_>>().join(" ");
    let result = run_ssh_workspace_command(
        config,
        password,
        "/",
        &format!(
            "test -d {} || exit 1; {commands}",
            shell_escape(destination)
        ),
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(30),
    )?;
    if result.exit_code != 0 {
        return Err(format!("无法检查上传目录：{}", result.stderr.trim()));
    }
    result
        .stdout
        .lines()
        .map(|line| {
            let (index, version) = line.split_once(' ').ok_or("同名文件检查结果无效")?;
            let name = names
                .get(index.parse::<usize>().map_err(|_| "同名文件序号无效")?)
                .ok_or("同名文件序号越界")?
                .clone();
            Ok(UploadConflict {
                name,
                version: version.into(),
            })
        })
        .collect()
}

pub fn ssh_browser_transfer_info(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
) -> Result<BrowserTransferInfo, String> {
    ssh_browser_transfer_info_with_access(config, password, path, &SshDownloadAccess::default())
}

pub fn ssh_browser_transfer_info_with_access(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
    access: &SshDownloadAccess,
) -> Result<BrowserTransferInfo, String> {
    transfer_info(
        config,
        password,
        path,
        access,
        Arc::new(AtomicBool::new(false)),
    )
}
fn transfer_info(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
    access: &SshDownloadAccess,
    cancellation: Arc<AtomicBool>,
) -> Result<BrowserTransferInfo, String> {
    let name = remote_name(path)?;
    // 一次往返同时拿到大小和类型：单个文件可以断点续传（直接读裸流），
    // 目录只能先远程打包。stat 失败时回显 unknown，调用方会退回到打包路径。
    let command = access.command(
        &format!(
            "du -sbL --count-links -- {path} && {{ stat -Lc '%F|%d:%i:%s:%Y:%z' -- {path} || echo unknown; }}",
            path = shell_escape(path)
        ),
        password,
    )?;
    let result = access.run(
        config,
        password,
        // Do not pipe through cut: its success would hide du failures (for
        // example an unreadable subtree returning only a partial size).
        &command,
        cancellation,
        Duration::from_secs(120),
    )?;
    transfer_info_result(name, result)
}

fn transfer_info_result(
    name: String,
    result: SshCommandResult,
) -> Result<BrowserTransferInfo, String> {
    // OpenSSH writes informational messages such as "Permanently added ..."
    // to stderr even when the remote command succeeds.
    if result.exit_code != 0 {
        return Err(super::download_access::download_failure(
            "无法计算下载大小",
            &result,
        ));
    }
    if result.output_truncated {
        return Err("服务器返回的文件大小信息不完整，未开始下载".into());
    }
    let mut lines = result.stdout.lines();
    let size_line = lines.next().unwrap_or_default();
    let detail = lines.next().unwrap_or_default().trim();
    let (kind, version) = detail
        .split_once('|')
        .map(|(kind, version)| (kind.trim(), version.trim()))
        .unwrap_or((detail, ""));
    // GNU du separates the count and path with a tab. The path may itself
    // contain whitespace, tabs or newlines; only parse the leading count.
    let bytes = size_line
        .split_once('\t')
        .map(|(count, _)| count)
        .unwrap_or(size_line)
        .trim()
        .parse::<u64>()
        .map_err(|_| "服务器没有返回有效的文件大小")?;
    Ok(BrowserTransferInfo {
        name,
        bytes,
        requires_confirmation: bytes > LARGE_DOWNLOAD_BYTES,
        // 只有明确是普通文件才走续传；其它情况（目录、设备文件、拿不到类型）
        // 一律走远程打包，那条路对目录和文件都成立。
        is_directory: !kind.eq_ignore_ascii_case("regular file"),
        version: version.to_string(),
    })
}

#[cfg(test)]
mod size_tests {
    use super::*;

    fn result(exit_code: i32, stdout: &str, stderr: &str) -> SshCommandResult {
        SshCommandResult {
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            output_truncated: false,
        }
    }

    #[test]
    fn download_size_accepts_success_with_openssh_host_key_notice() {
        let info = transfer_info_result("source".into(), result(0,
            "1073741825\t/work/source\n",
            "Warning: Permanently added '[10.179.85.15]:2202' (ED25519) to the list of known hosts.\r\n",
        )).unwrap();
        assert_eq!(info.bytes, LARGE_DOWNLOAD_BYTES + 1);
        assert!(info.requires_confirmation);
        assert_eq!(info.name, "source");
        let info =
            transfer_info_result("source".into(), result(0, "0\t/work/a\tb\nc\n", "")).unwrap();
        assert_eq!(info.bytes, 0);
        assert!(!info.requires_confirmation);
    }

    #[test]
    fn download_size_rejects_partial_counts_and_invalid_output() {
        let error = transfer_info_result(
            "source".into(),
            result(
                1,
                "42\t/work/source\n",
                "du: cannot read directory '/work/source/private': Permission denied\n",
            ),
        )
        .unwrap_err();
        assert!(error.contains("Permission denied"));
        assert!(
            transfer_info_result("source".into(), result(255, "42\t/work/source\n", ""))
                .unwrap_err()
                .contains("255")
        );
        for stdout in [
            "",
            "garbage\t/work/source\n",
            "-1\t/work/source\n",
            "18446744073709551616\t/work/source\n",
        ] {
            assert!(transfer_info_result("source".into(), result(0, stdout, "")).is_err());
        }
        let mut truncated = result(0, "42\t/work/source\n", "");
        truncated.output_truncated = true;
        assert!(transfer_info_result("source".into(), truncated).is_err());
    }

    #[test]
    fn transfer_info_reads_the_type_line_to_choose_resume_or_archive() {
        let file = transfer_info_result(
            "super.img".into(),
            result(
                0,
                "8589934592\t/work/super.img\nregular file|1234:99:8589934592:1761561897:8589934592\n",
                "",
            ),
        )
        .unwrap();
        assert!(!file.is_directory);
        assert_eq!(file.bytes, 8 * 1024 * 1024 * 1024);
        assert!(file.requires_confirmation);
        assert_eq!(file.version, "1234:99:8589934592:1761561897:8589934592");
        let directory = transfer_info_result(
            "logs".into(),
            result(0, "4096\t/work/logs\ndirectory\n", ""),
        )
        .unwrap();
        assert!(directory.is_directory);
        // stat 拿不到类型时回显 unknown：必须退回远程打包（目录和文件都成立）。
        let unknown =
            transfer_info_result("x".into(), result(0, "12\t/work/x\nunknown\n", "")).unwrap();
        assert!(unknown.is_directory);
        let missing = transfer_info_result("x".into(), result(0, "12\t/work/x\n", "")).unwrap();
        assert!(missing.is_directory);
    }

    #[test]
    fn resume_commands_continue_from_the_partial_size() {
        assert_eq!(
            resume_command("/work/super.img", 0),
            "cat -- '/work/super.img'"
        );
        assert_eq!(
            resume_command("/work/super.img", 1024),
            "tail -c +1025 -- '/work/super.img'"
        );
        assert_eq!(
            resume_command("/it's/a b.img", 0),
            "cat -- '/it'\\''s/a b.img'"
        );
    }

    #[test]
    fn resume_offsets_are_only_trusted_for_the_same_remote_version() {
        let info = BrowserTransferInfo {
            name: "super.img".into(),
            bytes: 8 * 1024 * 1024 * 1024,
            requires_confirmation: true,
            is_directory: false,
            version: "1:2:3:4:5".into(),
        };
        assert_eq!(resume_offset(&info, 0, ""), 0);
        assert_eq!(resume_offset(&info, 1024, "1:2:3:4:5"), 1024);
        assert_eq!(resume_offset(&info, 1024, " 1:2:3:4:5\n"), 1024);
        // 远端文件被重新编译过：旧断点不可信。
        assert_eq!(resume_offset(&info, 1024, "9:9:9:9:9"), 0);
        assert_eq!(resume_offset(&info, 1024, ""), 0);
        // 断点比远端还大：越界，从头来。
        assert_eq!(resume_offset(&info, info.bytes + 1, "1:2:3:4:5"), 0);
        // 刚好等于远端大小是合法的「已经传完」状态。
        assert_eq!(resume_offset(&info, info.bytes, "1:2:3:4:5"), info.bytes);
    }
}

fn remote_name(path: &str) -> Result<String, String> {
    if !path.starts_with('/') || path.contains('\0') || path.split('/').any(|part| part == "..") {
        return Err("传输路径必须是服务器绝对路径".into());
    }
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .ok_or("请选择具体的文件或目录")?;
    if !safe_transfer_name(name) {
        return Err("该名称不能用于 Windows 文件传输，请先重命名".into());
    }
    Ok(name.into())
}

struct RemoteStage {
    config: SshConnectionConfig,
    password: Option<String>,
    path: String,
    keep: bool,
}
impl RemoteStage {
    fn create(
        config: &SshConnectionConfig,
        password: Option<&str>,
        parent: Option<&str>,
        cancellation: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let template = parent
            .map(|parent| {
                format!(
                    "{}/.android-tool-transfer.XXXXXX",
                    parent.trim_end_matches('/')
                )
            })
            .unwrap_or("/tmp/android-tool-transfer.XXXXXX".into());
        let result = run_ssh_workspace_command(
            config,
            password,
            "/",
            &format!("mktemp -d -- {}", shell_escape(&template)),
            cancellation,
            Duration::from_secs(30),
        )?;
        let path = result.stdout.trim().to_string();
        let prefix = template.trim_end_matches("XXXXXX");
        if result.exit_code != 0
            || !path.starts_with(prefix)
            || !path[prefix.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
            || path.len() != prefix.len() + 6
        {
            return Err(format!("无法准备远程传输目录：{}", result.stderr.trim()));
        }
        Ok(Self {
            config: config.clone(),
            password: password.map(str::to_string),
            path,
            keep: false,
        })
    }
}
impl Drop for RemoteStage {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        // The exact mktemp result is validated above, never a user-provided path.
        let _ = run_ssh_workspace_command(
            &self.config,
            self.password.as_deref(),
            "/",
            &format!("rm -rf -- {}", shell_escape(&self.path)),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(15),
        );
    }
}

pub fn download_ssh_browser_path(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
    destination: &Path,
    confirmed_large: bool,
    cancellation: Arc<AtomicBool>,
    progress: TransferProgress,
) -> Result<(), String> {
    download_ssh_browser_path_with_access(
        config,
        password,
        path,
        destination,
        confirmed_large,
        &SshDownloadAccess::default(),
        cancellation,
        progress,
    )
}

pub fn download_ssh_browser_path_with_access(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
    destination: &Path,
    confirmed_large: bool,
    access: &SshDownloadAccess,
    cancellation: Arc<AtomicBool>,
    progress: TransferProgress,
) -> Result<(), String> {
    progress("checking", 0, 0);
    let info = transfer_info(config, password, path, access, cancellation.clone())?;
    if info.requires_confirmation && !confirmed_large {
        return Err("下载超过 1 GB，需要再次确认".into());
    }
    cancelled(&cancellation)?;
    // 单个普通文件走可续传的裸流：断线/取消之后再次点下载会从已传的字节接着来
    // （8 GB 的 super.img 不必每次从头）。需要 sudo 的路径没法走裸流（密码要走
    // stdin 给前一条命令），继续用远程打包那条稳的路。
    if !info.is_directory && !access.elevated {
        return download_file_resumable(
            config,
            password,
            path,
            destination,
            &info,
            cancellation,
            progress,
        );
    }
    let stage = RemoteStage::create(config, password, None, cancellation.clone())?;
    let remote_archive = format!("{}/payload.tar.gz", stage.path);
    let parent = remote_parent_path(path);
    progress("compressing", 0, info.bytes);
    let tar = access.command(
        &format!(
            "tar --dereference --hard-dereference -czf {} -C {} -- {}",
            shell_escape(&remote_archive),
            shell_escape(parent),
            shell_escape(&info.name),
        ),
        password,
    )?;
    // The login user owns the archive even when sudo reads its source files;
    // streaming and cleanup therefore need no additional elevated commands.
    let command = format!(
        ": > {archive} && {tar} && stat -c %s -- {archive}",
        archive = shell_escape(&remote_archive)
    );
    let result = access.run(
        config,
        password,
        &command,
        cancellation.clone(),
        Duration::from_secs(24 * 3600),
    )?;
    if result.exit_code != 0 {
        return Err(super::download_access::download_failure(
            "远程压缩失败",
            &result,
        ));
    }
    let size = result
        .stdout
        .trim()
        .parse::<u64>()
        .map_err(|_| "无法读取压缩大小")?;
    // Keep transport data outside the extracted tree, including when the downloaded
    // file itself is named like our payload archive.
    let archive = destination.with_file_name(format!(
        ".android-tool-download-{}.tar.gz",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&archive)
        .map_err(|error| error.to_string())?;
    struct LocalArchive(std::path::PathBuf);
    impl Drop for LocalArchive {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _archive_cleanup = LocalArchive(archive.clone());
    progress("transferring", 0, size);
    let callback = progress.clone();
    transfer_binary(
        config,
        password,
        &format!("cat -- {}", shell_escape(&remote_archive)),
        &archive,
        false,
        cancellation.clone(),
        Arc::new(move |bytes| callback("transferring", bytes, size)),
    )?;
    if fs::metadata(&archive)
        .map_err(|error| error.to_string())?
        .len()
        != size
    {
        return Err("压缩包传输不完整，未解压".into());
    }
    progress("extracting", 0, info.bytes);
    let callback = progress.clone();
    let result = unpack_transfer_archive(
        &archive,
        destination,
        cancellation,
        Arc::new(move |bytes| callback("extracting", bytes, info.bytes)),
    );
    let _ = fs::remove_file(&archive);
    result
}

/// 续传用的远端命令：`tail -c +N` 从第 N 个字节开始（1 基），所以偏移要 +1。
/// 写成单独函数是为了能对「从头」和「从断点」两条分支做单元测试。
fn resume_command(path: &str, offset: u64) -> String {
    let quoted = shell_escape(path);
    if offset == 0 {
        format!("cat -- {quoted}")
    } else {
        format!("tail -c +{} -- {quoted}", offset + 1)
    }
}

/// 决定从哪个偏移续传：0 表示从头来。断点只有在「同一份内容、长度不越界」时才可信。
fn resume_offset(info: &BrowserTransferInfo, length: u64, recorded: &str) -> u64 {
    if length == 0 || length > info.bytes {
        return 0;
    }
    if recorded.trim() != info.version {
        return 0;
    }
    length
}

/// 单个文件的断点续传。先看本地残留的 `.part` 有多大，再让远端从那个偏移继续发，
/// 全部到齐后才改名成目标文件；中途取消或断线时 `.part` 留在原处，下次接着传。
fn download_file_resumable(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
    destination: &Path,
    info: &BrowserTransferInfo,
    cancellation: Arc<AtomicBool>,
    progress: TransferProgress,
) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let target = destination.join(&info.name);
    let partial = destination.join(format!(".{}.part", info.name));
    let stamp = destination.join(format!(".{}.part.version", info.name));
    let length = fs::metadata(&partial)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .unwrap_or(0);
    // 断点必须属于同一份内容：远端文件被重新编译/替换过，接着旧偏移下载会得到
    // 一个前半段旧内容、后半段新内容的坏文件，所以版本对不上就从头来。
    let recorded = fs::read_to_string(&stamp).unwrap_or_default();
    let offset = resume_offset(info, length, &recorded);
    if offset == 0 && length > 0 {
        // 旧断点作废，别让追加写落在垃圾数据后面。
        let _ = fs::remove_file(&partial);
    }
    if info.bytes == 0 {
        fs::File::create(&target).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(&partial);
        let _ = fs::remove_file(&stamp);
        return Ok(());
    }
    if offset == info.bytes {
        // 上次其实已经传完，只是没来得及改名。
        replace_file(&partial, &target)?;
        let _ = fs::remove_file(&stamp);
        progress("transferring", info.bytes, info.bytes);
        return Ok(());
    }
    if !info.version.is_empty() {
        // 先落版本标记再传：中途崩溃时下一次才能判断断点还作不作数。
        fs::write(&stamp, &info.version).map_err(|error| error.to_string())?;
    }
    progress("transferring", offset, info.bytes);
    let callback = progress.clone();
    let base = offset;
    let total = info.bytes;
    append_binary(
        config,
        password,
        &resume_command(path, offset),
        &partial,
        cancellation.clone(),
        Arc::new(move |bytes| callback("transferring", base + bytes, total)),
    )?;
    let written = fs::metadata(&partial).map(|meta| meta.len()).unwrap_or(0);
    if written != info.bytes {
        return Err(format!(
            "下载不完整：已传 {} / {} 字节，断点已保留，再次下载会从断点继续",
            written, info.bytes
        ));
    }
    replace_file(&partial, &target)?;
    let _ = fs::remove_file(&stamp);
    progress("transferring", info.bytes, info.bytes);
    Ok(())
}

/// Windows 上 `rename` 遇到同名目标会直接失败，先删旧文件再改名。
fn replace_file(source: &Path, target: &Path) -> Result<(), String> {
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(source, target).map_err(|error| error.to_string())
}

pub fn upload_ssh_browser_paths(
    config: &SshConnectionConfig,
    password: Option<&str>,
    paths: &[String],
    destination: &str,
    local_archive: &Path,
    policy: UploadPolicy,
    conflicts: &[UploadConflict],
    cancellation: Arc<AtomicBool>,
    progress: TransferProgress,
) -> Result<(), String> {
    validate_destination(destination)?;
    progress("checking", 0, 0);
    let total = local_transfer_size(paths, &cancellation)?;
    let names = upload_names(paths)?;
    // Check before upload, then again just before installing extracted files.
    let guards = names.iter().map(|name| {
        let path = shell_escape(&format!("{}/{name}", destination.trim_end_matches('/')));
        if matches!(policy, UploadPolicy::Rename) { return String::new(); }
        if matches!(policy, UploadPolicy::Overwrite) {
            if let Some(conflict) = conflicts.iter().find(|conflict| &conflict.name == name) {
                return format!("test \"$(stat -c '%d:%i:%s:%y:%z' -- {path})\" = {} || {{ printf '远程同名文件已变化，请重新确认\\n' >&2; exit 1; }};", shell_escape(&conflict.version));
            }
        }
        format!("if [ -e {path} ] || [ -L {path} ]; then printf '同名文件已存在，请重新选择覆盖或保留两份\\n' >&2; exit 1; fi;")
    }).collect::<Vec<_>>().join(" ");
    let result = run_ssh_workspace_command(
        config,
        password,
        "/",
        &format!("test -d {} && {{ {guards} :; }}", shell_escape(destination)),
        cancellation.clone(),
        Duration::from_secs(30),
    )?;
    if result.exit_code != 0 {
        return Err(format!("无法上传：{}", result.stderr.trim()));
    }
    progress("compressing", 0, total);
    let callback = progress.clone();
    pack_transfer_archive(
        paths,
        local_archive,
        cancellation.clone(),
        Arc::new(move |bytes| callback("compressing", bytes, total)),
    )?;
    let size = fs::metadata(local_archive)
        .map_err(|error| error.to_string())?
        .len();
    let mut stage = RemoteStage::create(config, password, Some(destination), cancellation.clone())?;
    let remote_archive = format!("{}/payload.tar.gz", stage.path);
    progress("transferring", 0, size);
    let callback = progress.clone();
    transfer_binary(
        config,
        password,
        &format!("umask 077; cat > {}", shell_escape(&remote_archive)),
        local_archive,
        true,
        cancellation.clone(),
        Arc::new(move |bytes| callback("transferring", bytes, size)),
    )?;
    progress("extracting", 0, total);
    let extracted = format!("{}/files", stage.path);
    let moves = names.iter().enumerate().map(|(index, name)| {
        let source = shell_escape(&format!("{extracted}/{name}"));
        let target = shell_escape(&format!("{}/{name}", destination.trim_end_matches('/')));
        match policy {
            UploadPolicy::Rename => format!("n=0; target={target}; while :; do mv -nT -- {source} \"$target\" || exit 1; test ! -e {source} && break; n=$((n+1)); test $n -le 10000 || exit 1; target={target}.\"$n\"; done;"),
            UploadPolicy::Overwrite if conflicts.iter().any(|conflict| &conflict.name == name) => {
                // Move the old entry into our staging directory, so a failed install can restore it.
                let backup = shell_escape(&format!("{}/backup-{index}", stage.path));
                format!("mv -T -- {target} {backup} || exit 1; mv -nT -- {source} {target}; if [ -e {source} ]; then mv -nT -- {backup} {target}; printf '覆盖失败，请检查远程目录\\n' >&2; exit 1; fi;")
            }
            _ => format!("mv -nT -- {source} {target} || exit 1; test ! -e {source} || {{ printf '同名文件已出现，未覆盖\\n' >&2; exit 1; }};"),
        }
    }).collect::<Vec<_>>().join(" ");
    let command = format!("test \"$(stat -c %s -- {})\" = {size} && mkdir {} && tar -xzf {} --no-same-owner -C {} && {{ {guards} {moves} }}", shell_escape(&remote_archive), shell_escape(&extracted), shell_escape(&remote_archive), shell_escape(&extracted));
    // Keep backups if cancellation or a concurrent writer prevents restoring an old entry.
    stage.keep = matches!(policy, UploadPolicy::Overwrite) && !conflicts.is_empty();
    let result = run_ssh_workspace_command(
        config,
        password,
        "/",
        &command,
        cancellation,
        Duration::from_secs(24 * 3600),
    );
    let result = result.map_err(|error| {
        format!(
            "{error}{}",
            if stage.keep {
                format!("；恢复目录：{}", stage.path)
            } else {
                String::new()
            }
        )
    })?;
    if result.exit_code != 0 {
        return Err(format!(
            "远程解压或安装失败：{}{}",
            result.stderr.trim(),
            if stage.keep {
                format!("；原文件备份保留在 {}", stage.path)
            } else {
                String::new()
            }
        ));
    }
    stage.keep = false;
    Ok(())
}

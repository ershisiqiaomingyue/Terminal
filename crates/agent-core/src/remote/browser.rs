//! Read-only desktop file browsing, sharing the configured SSH transport.
use super::paths::{remote_path_is_within_root, remote_shell_path_expression};
use super::*;

pub(super) const MAX_BROWSER_FILE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshBrowserEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub kind: String,
    pub size: Option<u64>,
    pub symlink_target: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SshBrowserDirectory {
    pub path: String,
    pub entries: Vec<SshBrowserEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFileMetadata {
    pub size: u64,
    pub modified_at: Option<u64>,
    pub created_at: Option<u64>,
}

pub(super) fn browser_command(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    command: &str,
) -> Result<String, String> {
    let result = run_ssh_workspace_command(
        config,
        password,
        root,
        command,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(config.operation_timeout_secs.clamp(10, 120)),
    )?;
    if result.exit_code != 0 {
        return Err(format!("无法读取远程路径：{}", result.stderr.trim()));
    }
    if result.output_truncated {
        return Err("远程目录输出过大，请打开更具体的子目录".into());
    }
    Ok(result.stdout)
}

fn decode_browser_payload(encoded: &str) -> Result<Vec<u8>, String> {
    let encoded: String = encoded
        .chars()
        .filter(|value| !value.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("远程内容解码失败：{error}"))
}

fn parse_browser_directory(path: &str, bytes: &[u8]) -> Result<Vec<SshBrowserEntry>, String> {
    let mut fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.last() == Some(&&[][..]) {
        fields.pop();
    }
    if fields.len() % 5 != 0 {
        return Err("远程目录返回了不完整的数据".into());
    }
    if fields.len() / 5 > MAX_REMOTE_DIRECTORY_ENTRIES {
        return Err(format!(
            "目录超过 {MAX_REMOTE_DIRECTORY_ENTRIES} 项，请打开更具体的子目录"
        ));
    }
    let mut entries = Vec::new();
    for fields in fields.chunks_exact(5) {
        let name = std::str::from_utf8(fields[1]).map_err(|_| "远程文件名不是 UTF-8")?;
        if name.is_empty() || matches!(name, "." | "..") || name.contains('/') {
            return Err("远程目录返回了无效的文件名".into());
        }
        let effective_type = if fields[0] == b"l" {
            fields[3]
        } else {
            fields[0]
        };
        let kind = match effective_type {
            b"d" => "directory",
            b"f" => "file",
            _ if fields[0] == b"l" => "symlink",
            _ => "other",
        };
        entries.push(SshBrowserEntry {
            name: name.to_string(),
            path: format!("{}/{name}", path.trim_end_matches('/')),
            is_directory: kind == "directory",
            kind: kind.into(),
            size: std::str::from_utf8(fields[2])
                .ok()
                .and_then(|value| value.parse().ok()),
            symlink_target: if fields[0] == b"l" {
                Some(
                    std::str::from_utf8(fields[4])
                        .map_err(|_| "链接目标不是 UTF-8")?
                        .to_string(),
                )
            } else {
                None
            },
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

pub fn list_ssh_browser_directory(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
) -> Result<SshBrowserDirectory, String> {
    let mut requested = path.trim().to_string();
    // 空 = 家目录（与执行层「空 root 按登录目录」语义一致）；只有非空的相对
    // 路径才是真正的输入错误。
    if requested.is_empty() {
        requested = "~".to_string();
    }
    if !requested.starts_with('/') && !is_symbolic_remote_path(&requested) {
        return Err("请输入 SSH 绝对目录或 ~/ 路径".into());
    }
    let config = browser_connection(config);
    try_connection_operation(&config, browser_lock_wait(&config), || {
        let expression = remote_shell_path_expression(&requested)?;
        // GNU find emits metadata without reading file bodies; NUL framing retains
        // spaces, tabs and newlines in filenames, unlike line-based shell listings.
        let command = format!(
            "data=$({{ cd -L {expression} && {{ printf '%s\\0' \"$PWD\"; find . -mindepth 1 -maxdepth 1 -printf '%y\\0%f\\0%s\\0%Y\\0%l\\0'; }} || printf '\\0READ_ERROR\\0'; }} | head -z -n {} | base64) || exit 1; printf '%s' \"$data\"",
            (MAX_REMOTE_DIRECTORY_ENTRIES + 1) * 5 + 1,
        );
        let encoded = browser_command(&config, password, "/", &command)?;
        let bytes = decode_browser_payload(&encoded)?;
        if bytes.ends_with(b"\0READ_ERROR\0") {
            return Err("无法访问远程目录，请检查路径和读取权限".into());
        }
        let boundary = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("远程目录路径数据不完整")?;
        let path = std::str::from_utf8(&bytes[..boundary])
            .map_err(|_| "远程目录不是 UTF-8")?
            .to_string();
        let entries = parse_browser_directory(&path, &bytes[boundary + 1..])?;
        Ok(SshBrowserDirectory { path, entries })
    })
}

/// 等同一 SSH 泳道锁的上限：与命令超时同档，保证界面不会无限等待。
pub(super) fn browser_lock_wait(config: &SshConnectionConfig) -> Duration {
    Duration::from_secs(config.operation_timeout_secs.clamp(10, 120))
}

pub(super) fn browser_connection(config: &SshConnectionConfig) -> SshConnectionConfig {
    // 真标记（libssh2 协商失败记忆）才需要把泳道粘到 ssh.exe；russh 引擎
    // 可用时泳道键在引擎侧归一，共享主连接（v3 §21.4 S3）。
    let fallback = is_openssh_transport_marked(config);
    let mut config = config.clone();
    // Keep browsing responsive while an Agent command owns its SSH session.
    config.id.push_str(":file-browser");
    if fallback {
        remember_openssh_transport(&config);
    }
    config
}

pub(super) fn validate_browser_file_path(root: &str, path: &str) -> Result<(), String> {
    let trimmed_root = root.trim_end_matches('/');
    let scope = if trimmed_root.is_empty() {
        "/"
    } else {
        trimmed_root
    };
    if !root.starts_with('/')
        || !path.starts_with('/')
        || root.chars().chain(path.chars()).any(|value| value == '\0')
        || root.split('/').any(|part| part == "..")
        || path.split('/').any(|part| part == "..")
        || !remote_path_is_within_root(path, scope)
    {
        return Err("文件不在当前 SSH 目录内，请重新打开目录".into());
    }
    Ok(())
}

fn browser_text(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > MAX_BROWSER_FILE_BYTES {
        return Err("文件超过 1 MB，请使用终端或外部编辑器查看".into());
    }
    if bytes.contains(&0) {
        return Err("该文件为二进制文件，无法预览".into());
    }
    String::from_utf8(bytes).map_err(|_| "该文件不是 UTF-8 文本，无法预览".into())
}

pub(super) fn browser_file_guard(root: &str, path: &str) -> String {
    format!(
        "realpath -e -- {root} >/dev/null || exit 1; target=$(realpath -e -- {path} && printf '.') || exit 1; target=${{target%.}}; target=${{target%?}}; if [ ! -f {path} ] || [ ! -r {path} ]; then printf '只能打开可读的普通文本文件\\n' >&2; exit 1; fi; ",
        root = shell_escape(root), path = shell_escape(path),
    )
}

pub fn ssh_browser_file_version(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    path: &str,
) -> Result<String, String> {
    validate_browser_file_path(root, path)?;
    let config = browser_connection(config);
    try_connection_operation(&config, browser_lock_wait(&config), || {
        let command = format!(
            "{}stat -Lc '%d:%i:%s:%y:%z' -- {}",
            browser_file_guard(root, path),
            shell_escape(path)
        );
        browser_command(&config, password, root, &command)
            .map(|version| version.trim_end_matches(['\r', '\n']).to_string())
    })
}

pub fn ssh_browser_file_metadata(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    path: &str,
) -> Result<BrowserFileMetadata, String> {
    validate_browser_file_path(root, path)?;
    let config = browser_connection(config);
    try_connection_operation(&config, browser_lock_wait(&config), || {
        let command = format!(
            "{}stat -Lc '%s:%Y:%W' -- {}",
            browser_file_guard(root, path),
            shell_escape(path)
        );
        let output = browser_command(&config, password, root, &command)?;
        let mut fields = output.trim().split(':');
        let parse_seconds = |value: Option<&str>| {
            value
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|value| *value >= 0)
                .map(|value| value as u64 * 1000)
        };
        let size = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or("远程文件大小格式无效")?;
        let modified_at = parse_seconds(fields.next());
        let created_at = parse_seconds(fields.next()).filter(|value| *value > 0);
        Ok(BrowserFileMetadata {
            size,
            modified_at,
            created_at,
        })
    })
}

pub fn read_ssh_browser_file(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    path: &str,
) -> Result<String, String> {
    validate_browser_file_path(root, path)?;
    let config = browser_connection(config);
    try_connection_operation(&config, browser_lock_wait(&config), || {
        let command = format!(
            "{guard}data=$({{ head -c {limit} -- {path} || printf '\\0READ_ERROR\\0'; }} | base64) || exit 1; printf '%s' \"$data\"",
            guard = browser_file_guard(root, path),
            path = shell_escape(path), limit = MAX_BROWSER_FILE_BYTES + 1,
        );
        let encoded = browser_command(&config, password, root, &command)?;
        browser_text(decode_browser_payload(&encoded)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_preserves_names_and_sorts_directories_first() {
        let entries = parse_browser_directory(
            "/home/project",
            b"f\0a\tb\nc.md\012\0f\0\0d\0src\00\0d\0\0l\0link\00\0N\0missing\0",
        )
        .unwrap();
        assert_eq!(entries[0].path, "/home/project/src");
        assert_eq!(entries[1].name, "a\tb\nc.md");
        assert_eq!(entries[2].kind, "symlink");
        assert_eq!(entries[2].symlink_target.as_deref(), Some("missing"));
        assert!(entries[0].symlink_target.is_none());
        assert!(parse_browser_directory("/", b"f\0../bad\01\0f\0\0").is_err());
        assert!(parse_browser_directory("/", b"f\0partial").is_err());
    }
    #[test]
    fn browser_limits_and_scope_are_explicit() {
        assert!(validate_browser_file_path("/home/p", "/home/p/a").is_ok());
        assert!(validate_browser_file_path("/", "/tmp/a").is_ok());
        for path in ["/home/p2/a", "/home/p/../a", "C:/test"] {
            assert!(validate_browser_file_path("/home/p", path).is_err());
        }
        assert_eq!(browser_text(b"hi\n\n".to_vec()).unwrap(), "hi\n\n");
        assert!(browser_text(vec![0]).is_err());
        assert!(browser_text(vec![0xff]).is_err());
        assert!(browser_text(vec![b'a'; MAX_BROWSER_FILE_BYTES + 1]).is_err());
        assert!(parse_browser_directory(
            "/",
            &b"f\0a\01\0f\0\0".repeat(MAX_REMOTE_DIRECTORY_ENTRIES + 1)
        )
        .is_err());
    }
    #[test]
    fn browser_links_retain_targets_and_follow_target_type() {
        let entries = parse_browser_directory("/p", b"l\0workspace\01\0d\0../project -> other\0l\0file.rs\01\0f\0src/a\tb\nc.rs\0l\0broken\00\0N\0missing\0l\0cycle\00\0L\0cycle\0").unwrap();
        let directory = entries
            .iter()
            .find(|entry| entry.name == "workspace")
            .unwrap();
        assert!(directory.is_directory);
        assert_eq!(
            directory.symlink_target.as_deref(),
            Some("../project -> other")
        );
        let file = entries
            .iter()
            .find(|entry| entry.name == "file.rs")
            .unwrap();
        assert_eq!(file.kind, "file");
        assert_eq!(file.symlink_target.as_deref(), Some("src/a\tb\nc.rs"));
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.name == "cycle")
                .unwrap()
                .kind,
            "symlink"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated SSH fixture"]
    fn real_ssh_browser_refresh_and_git() {
        let distribution = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
        let fixture = ssh_fixture::SshFixture::start(&distribution);
        let config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
        let preparation = "mkdir project; cd project; mkdir src; printf 'first\\n' > src/main.rs; printf '# 文档\\n\\n正文\\n\\n' > 'a b.md'; printf 'special\\n' > 'tab\tline\n.md'; printf '\\0binary' > payload.bin; head -c 1048577 /dev/zero | tr '\\0' a > large.txt; git init -q; git add src/main.rs 'a b.md'; git -c user.name=测试作者 -c user.email=test@example.invalid commit -qm initial; printf 'second\\nthird\\n' > src/main.rs";
        let result = run_ssh_workspace_command(
            &config,
            None,
            &fixture.remote_root,
            preparation,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        let root = format!("{}/project", fixture.remote_root);
        for fallback in [false, true] {
            if fallback {
                remember_openssh_transport(&config);
                remember_openssh_transport(&browser_connection(&config));
            }
            let directory = list_ssh_browser_directory(&config, None, &root).unwrap();
            assert_eq!(directory.path, root);
            assert!(directory.entries[0].is_directory);
            assert!(directory
                .entries
                .iter()
                .any(|entry| entry.name == "tab\tline\n.md"));
            let links = run_ssh_workspace_command(
                &config,
                None,
                &root,
                "ln -sfn src linked-src; ln -sfn 'a b.md' linked.md",
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(30),
            )
            .unwrap();
            assert_eq!(links.exit_code, 0);
            let linked = list_ssh_browser_directory(&config, None, &root).unwrap();
            assert!(
                linked
                    .entries
                    .iter()
                    .find(|entry| entry.name == "linked-src")
                    .unwrap()
                    .is_directory
            );
            assert_eq!(
                linked
                    .entries
                    .iter()
                    .find(|entry| entry.name == "linked-src")
                    .unwrap()
                    .symlink_target
                    .as_deref(),
                Some("src")
            );
            assert_eq!(
                linked
                    .entries
                    .iter()
                    .find(|entry| entry.name == "linked.md")
                    .unwrap()
                    .symlink_target
                    .as_deref(),
                Some("a b.md")
            );
            let linked_dir =
                list_ssh_browser_directory(&config, None, &format!("{root}/linked-src")).unwrap();
            assert_eq!(linked_dir.path, format!("{root}/linked-src"));
            assert_eq!(
                read_ssh_browser_file(&config, None, &root, &format!("{root}/linked.md")).unwrap(),
                "# 文档\n\n正文\n\n"
            );
            let outside = run_ssh_workspace_command(&config, None, &root, "mkdir -p ../outside; printf 'outside\\n' > ../outside/file.rs; ln -sfn ../outside external", Arc::new(AtomicBool::new(false)), Duration::from_secs(30)).unwrap();
            assert_eq!(outside.exit_code, 0);
            assert_eq!(
                read_ssh_browser_file(&config, None, &root, &format!("{root}/external/file.rs"))
                    .unwrap(),
                "outside\n"
            );
            let link_version =
                ssh_browser_file_version(&config, None, &root, &format!("{root}/linked.md"))
                    .unwrap();
            assert_eq!(
                link_version,
                ssh_browser_file_version(&config, None, &root, &format!("{root}/a b.md")).unwrap()
            );
            assert_eq!(
                read_ssh_browser_file(&config, None, &root, &format!("{root}/a b.md")).unwrap(),
                "# 文档\n\n正文\n\n"
            );
            assert_eq!(
                read_ssh_browser_file(&config, None, &root, &format!("{root}/tab\tline\n.md"))
                    .unwrap(),
                "special\n"
            );
            assert!(
                read_ssh_browser_file(&config, None, &root, &format!("{root}/payload.bin"))
                    .is_err()
            );
            assert!(
                read_ssh_browser_file(&config, None, &root, &format!("{root}/large.txt")).is_err()
            );
            assert!(list_ssh_browser_directory(&config, None, &format!("{root}/missing")).is_err());
            let path = format!("{root}/src/main.rs");
            let before = ssh_browser_file_version(&config, None, &root, &path).unwrap();
            let command = format!("printf 'changed\\n' > {}", shell_escape(&path));
            let output = run_ssh_workspace_command(
                &config,
                None,
                &root,
                &command,
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(30),
            )
            .unwrap();
            assert_eq!(output.exit_code, 0);
            assert_ne!(
                ssh_browser_file_version(&config, None, &root, &path).unwrap(),
                before
            );
            let git = ssh_git_browser_directory(&config, None, &root)
                .unwrap()
                .unwrap();
            let changed = git.files.iter().find(|file| file.path == path).unwrap();
            assert!(changed.status.contains('M'));
            assert_eq!(changed.additions, Some(1));
            assert_eq!(changed.deletions, Some(1));
            let details = ssh_git_browser_details(&config, None, &root, &path).unwrap();
            assert_eq!(details.author.as_deref(), Some("测试作者"));
            assert_eq!(details.repo_root.as_deref(), Some(root.as_str()));
            assert_eq!(details.file.as_ref().unwrap().additions, Some(1));
            assert_eq!(details.file.as_ref().unwrap().deletions, Some(1));
            assert!(details.diff.contains("+changed"));
            assert!(
                ssh_git_browser_directory(&config, None, &fixture.remote_root)
                    .unwrap()
                    .is_none()
            );
            let version = ssh_browser_file_version(&config, None, &root, &path).unwrap();
            let edited = "远程编辑\n".repeat(32 * 1024);
            let saved =
                write_ssh_browser_file(&config, None, &root, &path, &edited, &version).unwrap();
            assert_ne!(saved, version);
            assert_eq!(
                read_ssh_browser_file(&config, None, &root, &path).unwrap(),
                edited
            );
            let conflict =
                write_ssh_browser_file(&config, None, &root, &path, "must not overwrite", &version)
                    .unwrap_err();
            assert!(conflict.contains("未覆盖保存"), "{conflict}");
            assert_eq!(
                read_ssh_browser_file(&config, None, &root, &path).unwrap(),
                edited
            );
            let linked_path = format!("{root}/linked.md");
            let version = ssh_browser_file_version(&config, None, &root, &linked_path).unwrap();
            write_ssh_browser_file(
                &config,
                None,
                &root,
                &linked_path,
                "# 文档\n\n正文\n\n",
                &version,
            )
            .unwrap();
            let result = run_ssh_workspace_command(&config, None, &root, "test -L linked.md && test -z \"$(find . -name '.android-tool-save.*' -print -quit)\"", Arc::new(AtomicBool::new(false)), Duration::from_secs(30)).unwrap();
            assert_eq!(
                result.exit_code, 0,
                "save must preserve links and clean staging files"
            );
            assert!(write_ssh_browser_file(
                &config,
                None,
                &root,
                &path,
                &"a".repeat(MAX_BROWSER_FILE_BYTES + 1),
                &saved
            )
            .is_err());
        }
        let cleanup = run_ssh_workspace_command(&config, None, &fixture.remote_root, "test -d project && test ! -L project && test -d outside && test ! -L outside && rm -rf -- project outside", Arc::new(AtomicBool::new(false)), Duration::from_secs(30)).unwrap();
        assert_eq!(cleanup.exit_code, 0);
    }
}

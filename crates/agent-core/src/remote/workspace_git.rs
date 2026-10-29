//! 远端工作区 Git 概况（右栏「版本控制」页签在 SSH 页签下使用）。
//!
//! 与 `git_browser` 的区别：那份返回的是代码查看器用的目录树形状
//! （`GitBrowserDirectory` / 绝对路径文件项），这里要的是右栏 GitPane 的形状
//! ——**与本地 `get_workspace_environment` / `get_workspace_git_changes` 完全一致**
//! 的 `WorkspaceGitSummary` / `WorkspaceGitChange`，前端两处走同一套渲染与缓存。
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use super::connection::is_openssh_transport_marked;
use super::{remember_openssh_transport, run_ssh_workspace_command, SshConnectionConfig};

/// 单条 git 子命令超时；总耗时 = 3 条 × 该值（首次连接可能还要认证）。
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 「登录目录」占位值。`run_ssh_workspace_command` 走到 `cd` 时会把路径整体
/// 单引号转义，`~` 与 `$HOME` 都会变成字面量（实机 `sh` 报 `can't cd to ~`），
/// 所以需要登录目录时传这个哨兵，由执行层翻译成不带参数的 `cd`。
///
/// **必须是纯 ASCII 且不含 NUL**：ssh 客户端在组装远端命令行时把参数当 C 字符串，
/// 含 NUL 会被截断（调用方在文件末尾追加的 shell 片段整段丢失，表现为 exit=1、
/// stderr 为空，排查了很久）。用 GIT_CWD 打头还方便在 `git` 无关日志里一眼认出。
pub(super) const LOGIN_DIRECTORY_MARKER: &str = "__git_cwd_login_directory__";

/// 判断一个 root 是否就是哨兵值（执行层与解析层共用，避免两处字符串写错）。
pub(super) fn is_login_directory_marker(root: &str) -> bool {
    root.trim() == LOGIN_DIRECTORY_MARKER
}

/// 把前端传来的 root 规范化成可以交给 `run_ssh_workspace_command` 的 cwd。
///
/// - 空 root 或 `~` / `$HOME`：返回 `LOGIN_DIRECTORY_MARKER`（登录目录）；
/// - 其余原样透传字面路径。
///
/// **不能**直接回 `/home/xxx` 之类的绝对路径：远端家目录未必在 `/home`，
/// 而且 `~` 本身在经 `shell_escape` 之后不展开。
fn remote_root_cwd(root: &str) -> &str {
    let value = root.trim();
    if value.is_empty() || value == "~" || value == "~/." || value == "$HOME" {
        LOGIN_DIRECTORY_MARKER
    } else {
        value
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitSummary {
    pub available: bool,
    pub repository: bool,
    pub branch: String,
    pub dirty: bool,
    pub changed_files: u64,
    pub added_lines: u64,
    pub removed_lines: u64,
    /// 仓库根（绝对路径）；不是仓库时回填请求的 root，方便前端提示"哪个目录不是仓库"。
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitChange {
    pub path: String,
    pub status: String,
    pub added_lines: u64,
    pub removed_lines: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshWorkspaceGit {
    pub summary: WorkspaceGitSummary,
    pub changes: Vec<WorkspaceGitChange>,
}

impl WorkspaceGitSummary {
    /// 与前端 `readWorkspaceGit` 的兜底形状一致：字段都在，只是标成不可用。
    fn placeholder(path: &str, available: bool) -> Self {
        Self {
            available,
            repository: false,
            branch: String::new(),
            dirty: false,
            changed_files: 0,
            added_lines: 0,
            removed_lines: 0,
            path: path.to_string(),
        }
    }
}

/// 与本地 `src-tauri/src/lib.rs` 的 `git_status_label` 同一张映射表：
/// 前端按状态串上色，两处必须一致，否则同一份改动在本地/SSH 下颜色不同。
pub fn git_status_label(code: &str) -> &'static str {
    if code == "??" {
        "created"
    } else if code.contains('D') {
        "deleted"
    } else if code.contains('R') {
        "renamed"
    } else {
        "modified"
    }
}

/// 解析 `git status --porcelain=v1 -b` 的分支头。
///
/// 形如 `main`、`main...origin/main`、`main...origin/main [ahead 1, behind 2]`、
/// `HEAD (no branch)`、`No commits yet on main`。口径与本地一致：去掉上游与
/// ahead/behind 段，分离 HEAD 统一显示「分离 HEAD」。
pub fn parse_branch_header(line: &str) -> Option<String> {
    // `status -z` 的分支记录以 NUL 结尾，`lines()` 不会切掉它，必须自己剥，
    // 否则分支名会带出 `\u{0}`（实测 branch = "stable\u{0}"）。
    let value = line
        .strip_prefix("## ")?
        .trim_matches(|value: char| value == '\0' || value.is_whitespace());
    if value.is_empty() {
        return None;
    }
    if value.starts_with("HEAD (no branch)") {
        return Some("分离 HEAD".to_string());
    }
    if let Some(rest) = value.strip_prefix("No commits yet on ") {
        let rest = rest.trim();
        return (!rest.is_empty()).then(|| rest.to_string());
    }
    // `main...origin/main [ahead 1]`：上游与跟踪信息都不进分支名。
    let value = value.split("...").next().unwrap_or(value).trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 解析 `git diff --numstat -z`。
///
/// 重命名记录形如 `12\t3\t\0old\0new\0`（第三段为空表示后面还有两个 NUL 字段），
/// 取新路径；二进制文件的计数列是 `-`，按 0 处理（与本地 `unwrap_or(0)` 一致）。
pub fn parse_numstat(output: &str) -> Vec<(String, u64, u64)> {
    let mut rows = Vec::new();
    let mut fields = output.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let added_lines = added.parse::<u64>().unwrap_or(0);
        let removed_lines = removed.parse::<u64>().unwrap_or(0);
        let path = if path.is_empty() {
            let _origin = fields.next();
            match fields.next() {
                Some(new_path) if !new_path.is_empty() => new_path,
                _ => continue,
            }
        } else {
            path
        };
        rows.push((path.to_string(), added_lines, removed_lines));
    }
    rows
}

/// 解析 `git status --porcelain=v1 -z` 的变更记录（跳过首行 `## ...`）。
///
/// 返回 (状态码两字符, 相对路径)。重命名/复制的记录后面还跟一个原始路径字段，
/// 必须一并吃掉，否则那一段会被误当成另一个变更。
pub fn parse_status_records(output: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut fields = output.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() || field.starts_with("## ") {
            continue;
        }
        // `XY path`：两位状态码 + 空格。不足 4 字节的是误投字段，跳过。
        if field.len() < 4 || !field.is_char_boundary(2) {
            continue;
        }
        let (code, rest) = field.split_at(2);
        let path = rest.strip_prefix(' ').unwrap_or(rest);
        if code.contains('R') || code.contains('C') {
            let _origin = fields.next();
        }
        if path.is_empty() {
            continue;
        }
        rows.push((code.to_string(), path.to_string()));
    }
    rows
}

/// 由 git 原始输出组装右栏 Git 面板数据（纯函数，便于单测）。
///
/// 「是不是仓库」由调用方用 `rev-parse --show-toplevel` 判定，这里只管形状。
pub fn build_workspace_git(
    root: &str,
    branch_source: Option<&str>,
    status: &str,
    numstat: &str,
) -> SshWorkspaceGit {
    let branch = branch_source
        .and_then(parse_branch_header)
        .unwrap_or_default();
    let rows = parse_numstat(numstat);
    let (added_lines, removed_lines) = rows.iter().fold((0u64, 0u64), |(add, rem), (_, a, r)| {
        (add.saturating_add(*a), rem.saturating_add(*r))
    });
    let counts: BTreeMap<&str, (u64, u64)> = rows
        .iter()
        .map(|(path, a, r)| (path.as_str(), (*a, *r)))
        .collect();
    let mut changes: Vec<WorkspaceGitChange> = parse_status_records(status)
        .into_iter()
        .map(|(code, path)| {
            let (added, removed) = counts.get(path.as_str()).copied().unwrap_or((0, 0));
            WorkspaceGitChange {
                status: git_status_label(&code).to_string(),
                path,
                added_lines: added,
                removed_lines: removed,
            }
        })
        .collect();
    // 与本地 `get_workspace_git_changes` 一致：按路径升序。
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    let changed_files = changes.len() as u64;
    SshWorkspaceGit {
        summary: WorkspaceGitSummary {
            available: true,
            repository: true,
            branch,
            // 「脏」以变更文件数为准：untracked 不计入 numstat，但确实属于改动。
            dirty: changed_files > 0,
            changed_files,
            added_lines,
            removed_lines,
            path: root.to_string(),
        },
        changes,
    }
}

/// 远端执行一条 git 子命令并取回原始 stdout。
///
/// 输出先 base64 再回传：porcelain -z 里可能有任意字节，直接按文本读会被
/// `from_utf8_lossy` 改写路径，前后端就对不上了。
///
/// **脚本必须写成单行、且不要用命令替换取 `$?`**（排查记录见下）。
fn git_query(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    args: &str,
) -> Result<String, String> {
    // 三条实机约束（10.179.85.15 / git 2.34.1，逐条踩过）：
    // 1. **必须单行**：执行层把整段命令拼在 `cd '…' && ` 后交给远端 `sh -c`，
    //    命令替换里的换行会让后面的语句成段丢失（曾表现为 stdout/stderr 全空、exit=1）；
    // 2. **不要用 `$( )` 包住 git 再取 `$?`**：实测命令替换的赋值读回来是空的，
    //    连带输出一起丢（`P3=`、`outlen=0`）；
    // 3. **退出码不可信**：管道尾部是 `tr`/`base64`，实测非仓库里也能报 exit=0。
    //    所以成功与否单独由 `git_succeeds` 判定，这里只负责取输出。
    let script = format!(
        "git --no-optional-locks -c core.quotepath=false {args} 2>/dev/null | base64 | tr -d '\n'",
        args = args,
    );
    let result = run_ssh_workspace_command(
        config,
        password,
        root,
        &script,
        Arc::new(AtomicBool::new(false)),
        GIT_TIMEOUT,
    )?;
    if result.output_truncated {
        return Err("Git 输出过大，请在更具体的子目录下查看".into());
    }
    let encoded = result.stdout.trim();
    if encoded.is_empty() {
        return Ok(String::new());
    }
    base64_decode(encoded).ok_or_else(|| "远端 Git 输出解码失败，请重试".to_string())
}

/// 单独探一次 git 子命令的退出码。
///
/// 取输出那条命令的退出码被管道吃掉，所以成功与否必须另起一条**不带管道**的命令：
/// 它的 stdout 是空的，退出码就是 git 自己的。
fn git_succeeds(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    args: &str,
) -> Result<bool, String> {
    let result = run_ssh_workspace_command(
        config,
        password,
        root,
        &format!("git --no-optional-locks -c core.quotepath=false {args} >/dev/null 2>&1"),
        Arc::new(AtomicBool::new(false)),
        GIT_TIMEOUT,
    )?;
    Ok(result.exit_code == 0)
}

/// 最小 base64 解码（标准字母表 + `=` 填充）。
///
/// 远端用 `base64` 是为了让 porcelain 里的任意字节原样过 SSH 通道；这里只需要
/// 解回原始字节，不引入额外依赖。非法字符或长度不对一律 `None`。
fn base64_decode(input: &str) -> Option<String> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a') as u32 + 26),
            b'0'..=b'9' => Some((byte - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let bytes = match bytes.iter().position(|byte| *byte == b'=') {
        Some(index) => &bytes[..index],
        None => &bytes[..],
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let mut buffer = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            buffer |= value(*byte)? << (18 - 6 * index);
        }
        out.push((buffer >> 16) as u8);
        if chunk.len() > 2 {
            out.push((buffer >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(buffer as u8);
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// 远端工作区 Git 概况。`root` 为空时按远端家目录（`~`）处理。
pub fn ssh_workspace_git(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
) -> Result<SshWorkspaceGit, String> {
    let root = remote_root_cwd(root);
    let display_root = if root.is_empty() { "~" } else { root };
    let fallback = is_openssh_transport_marked(config);
    // 与代码查看器的 Git 命令分开一条连接池键：两套视图的操作锁互不阻塞。
    // openssh 回退必须用**原始** id 注册（它只认服务器键），所以先注册再改 id。
    let mut config = config.clone();
    if fallback {
        remember_openssh_transport(&config);
    }
    config.id.push_str(":workspace-git");
    let config = &config;

    // 先确认是不是仓库；不是就返回 available=true + repository=false，
    // 前端据此显示「不是 Git 仓库」（与本地行为一致，不是错误）。
    if !git_succeeds(config, password, root, "rev-parse --show-toplevel")? {
        return Ok(SshWorkspaceGit {
            summary: WorkspaceGitSummary::placeholder(display_root, true),
            changes: Vec::new(),
        });
    }
    // 成功但读到空输出只可能是极罕见的竞态（目录刚好被删），按 root 兜底。
    let toplevel = git_query(config, password, root, "rev-parse --show-toplevel")?;
    let repo_root = {
        let value = toplevel.trim();
        if value.is_empty() {
            display_root.to_string()
        } else {
            value.to_string()
        }
    };

    // `-b` 让首行是 `## 分支`；`-z` 让路径不被引号转义。
    let status = if git_succeeds(
        config,
        password,
        root,
        "status --porcelain=v1 -z -b --untracked-files=normal --no-renames",
    )? {
        git_query(
            config,
            password,
            root,
            "status --porcelain=v1 -z -b --untracked-files=normal --no-renames",
        )?
    } else {
        String::new()
    };
    // 新仓库没有任何提交时 `diff HEAD` 失败 → 退回不带 HEAD 的工作区 diff。
    let numstat = if git_succeeds(
        config,
        password,
        root,
        "diff --numstat -z --no-renames HEAD",
    )? {
        git_query(
            config,
            password,
            root,
            "diff --numstat -z --no-renames HEAD",
        )?
    } else if git_succeeds(config, password, root, "diff --numstat -z --no-renames")? {
        // 新仓库没有任何提交时 `diff HEAD` 失败 → 退回不带 HEAD 的工作区 diff。
        git_query(config, password, root, "diff --numstat -z --no-renames")?
    } else {
        String::new()
    };

    // status 的首行是分支头；`-z` 下首行可能不带换行，按行取第一段即可。
    let branch_line = status.lines().next().unwrap_or_default();
    let mut data = build_workspace_git(&repo_root, Some(branch_line), &status, &numstat);
    data.summary.path = repo_root;
    Ok(data)
}

/// 查询远端当前工作目录（cwd 跟随的兜底通路）。
///
/// 只跑 `pwd`：存在 OSC 7 的 shell 会主动上报，只有 SSH 这种拿不到上报的
/// 场景才需要这个低频轮询。返回单个绝对路径，失败一律 Err。
pub fn query_ssh_cwd(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
) -> Result<String, String> {
    let root = remote_root_cwd(root);
    let fallback = is_openssh_transport_marked(config);
    let mut config = config.clone();
    if fallback {
        remember_openssh_transport(&config);
    }
    config.id.push_str(":cwd-probe");
    // root 为空/`~` 时走登录目录；否则先 `cd` 到该目录再 `pwd`——这条通路没有
    // 持久 shell，每条命令都是新进程，不 cd 就永远只能拿到登录目录。
    // 同样必须单行：多行脚本在远端 `sh -c` 下会被截断（见 `git_query` 的排查记录）。
    let result = run_ssh_workspace_command(
        &config,
        password,
        root,
        "pwd",
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(8),
    )?;
    if result.exit_code != 0 {
        return Err(if result.stderr.trim().is_empty() {
            "无法获取远端工作目录".into()
        } else {
            result.stderr.trim().to_string()
        });
    }
    let path = result.stdout.trim();
    if path.is_empty() {
        return Err("无法获取远端工作目录".into());
    }
    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_root_cwd_maps_home_forms_to_login_marker() {
        // `~` / `$HOME` / 空串都必须换成哨兵：执行层会整体加单引号，
        // 直接透传会被 `sh` 当成字面量路径（实机 `can't cd to ~`）。
        assert_eq!(remote_root_cwd(""), LOGIN_DIRECTORY_MARKER);
        assert_eq!(remote_root_cwd("   "), LOGIN_DIRECTORY_MARKER);
        assert_eq!(remote_root_cwd("~"), LOGIN_DIRECTORY_MARKER);
        assert_eq!(remote_root_cwd("~/."), LOGIN_DIRECTORY_MARKER);
        assert_eq!(remote_root_cwd("$HOME"), LOGIN_DIRECTORY_MARKER);
        // 其余路径原样透传，含中文与空格，交给 shell_escape 处理。
        assert_eq!(remote_root_cwd("/tmp/x"), "/tmp/x");
        assert_eq!(remote_root_cwd("/home/app1whu/proj"), "/home/app1whu/proj");
        assert_eq!(remote_root_cwd(" /home/a b/c "), "/home/a b/c");
    }

    #[test]
    fn branch_header_matches_local_semantics() {
        assert_eq!(parse_branch_header("## main").as_deref(), Some("main"));
        assert_eq!(
            parse_branch_header("## main...origin/main [ahead 1]").as_deref(),
            Some("main")
        );
        assert_eq!(
            parse_branch_header("## HEAD (no branch)").as_deref(),
            Some("分离 HEAD")
        );
        assert_eq!(
            parse_branch_header("## No commits yet on main").as_deref(),
            Some("main")
        );
        assert_eq!(parse_branch_header("## ").as_deref(), None);
        assert_eq!(parse_branch_header("nothing").as_deref(), None);
    }

    #[test]
    fn status_records_handle_renames_and_spaces() {
        let rows = parse_status_records("## main\0 M src/a b.rs\0?? new.txt\0R  old.rs\0orig.rs\0");
        assert_eq!(
            rows,
            vec![
                (" M".to_string(), "src/a b.rs".to_string()),
                ("??".to_string(), "new.txt".to_string()),
                ("R ".to_string(), "old.rs".to_string()),
            ]
        );
    }

    #[test]
    fn numstat_handles_binary_and_renames() {
        let rows = parse_numstat("3\t1\tsrc/a b.rs\0-\t-\timg.png\0");
        assert_eq!(rows[0], ("src/a b.rs".to_string(), 3, 1));
        assert_eq!(rows[1], ("img.png".to_string(), 0, 0));
        let renamed = parse_numstat("5\t2\t\0old.rs\0new.rs\0");
        assert_eq!(renamed, vec![("new.rs".to_string(), 5, 2)]);
    }

    #[test]
    fn build_matches_local_shape() {
        let data = build_workspace_git(
            "/repo",
            Some("## feature/x...origin/feature/x [ahead 2]"),
            "## feature/x...origin/feature/x [ahead 2]\0 M src/a b.rs\0?? new.txt\0",
            "3\t1\tsrc/a b.rs\0",
        );
        assert!(data.summary.available && data.summary.repository);
        assert_eq!(data.summary.branch, "feature/x");
        assert_eq!(data.summary.path, "/repo");
        assert_eq!(data.summary.changed_files, 2);
        assert_eq!(data.summary.added_lines, 3);
        assert_eq!(data.summary.removed_lines, 1);
        assert!(data.summary.dirty);
        // 按路径升序：new.txt < src/a b.rs。
        assert_eq!(data.changes[0].path, "new.txt");
        assert_eq!(data.changes[0].status, "created");
        assert_eq!(data.changes[0].added_lines, 0);
        assert_eq!(data.changes[1].status, "modified");
        assert_eq!(data.changes[1].added_lines, 3);
    }

    #[test]
    fn branch_header_strips_porcelain_nul_terminator() {
        // `git status --porcelain=v1 -z -b` 的首条记录是 `## main\0`，
        // 按行切分不会去掉 NUL，分支名必须自己剥干净。
        assert_eq!(parse_branch_header("## main\0"), Some("main".to_string()));
        assert_eq!(parse_branch_header("## main"), Some("main".to_string()));
        assert_eq!(
            parse_branch_header("## stable...origin/stable [ahead 1]\0"),
            Some("stable".to_string())
        );
        assert_eq!(parse_branch_header("## \0"), None);
    }

    #[test]
    fn clean_repository_reports_not_dirty() {
        let data = build_workspace_git("/repo", Some("## main"), "## main\0", "");
        assert!(data.summary.repository);
        assert!(!data.summary.dirty);
        assert_eq!(data.summary.changed_files, 0);
        assert!(data.changes.is_empty());
        let missing = WorkspaceGitSummary::placeholder("/tmp", true);
        assert!(!missing.repository && missing.available);
    }
}

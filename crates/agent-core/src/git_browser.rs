//! Read-only Git information for local and remote directory browsers.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBrowserFile {
    pub path: String,
    pub status: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct GitBrowserDirectory {
    pub root: String,
    pub files: Vec<GitBrowserFile>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBrowserDetails {
    pub repo_root: Option<String>,
    pub file: Option<GitBrowserFile>,
    pub author: Option<String>,
    pub committed_at: Option<String>,
    pub subject: Option<String>,
    pub diff: String,
}

pub(crate) fn git_directory(
    root: &str,
    status: &[u8],
    numstat: &[u8],
) -> Result<GitBrowserDirectory, String> {
    let mut counts = BTreeMap::new();
    let mut fields = numstat.split(|byte| *byte == 0);
    while let Some(field) = fields.next().filter(|field| !field.is_empty()) {
        let value = std::str::from_utf8(field).map_err(|_| "Git 路径不是 UTF-8")?;
        let mut parts = value.splitn(3, '\t');
        let added = parts.next().and_then(|value| value.parse::<u64>().ok());
        let removed = parts.next().and_then(|value| value.parse::<u64>().ok());
        let Some(path) = parts.next() else {
            return Err("Git 行数数据不完整".into());
        };
        let path = if path.is_empty() {
            fields.next().ok_or("Git 重命名数据不完整")?;
            std::str::from_utf8(fields.next().ok_or("Git 重命名数据不完整")?)
                .map_err(|_| "Git 路径不是 UTF-8")?
        } else {
            path
        };
        counts.insert(path.to_string(), (added, removed));
    }
    let mut files = Vec::new();
    let mut fields = status.split(|byte| *byte == 0);
    while let Some(field) = fields.next().filter(|field| !field.is_empty()) {
        let value = std::str::from_utf8(field).map_err(|_| "Git 路径不是 UTF-8")?;
        if value.len() < 4 || !value.is_char_boundary(3) {
            return Err("Git 状态数据不完整".into());
        }
        let state = &value[..2];
        let path = &value[3..];
        if state.contains('R') || state.contains('C') {
            fields
                .next()
                .filter(|field| !field.is_empty())
                .ok_or("Git 重命名数据不完整")?;
        }
        let (additions, deletions) = counts.get(path).copied().unwrap_or((None, None));
        files.push(GitBrowserFile {
            path: format!("{}/{}", root.trim_end_matches(['/', '\\']), path),
            status: state.into(),
            additions,
            deletions,
        });
    }
    Ok(GitBrowserDirectory {
        root: root.to_string(),
        files,
    })
}

pub(crate) fn git_details(log: &[u8], diff: &[u8]) -> Result<GitBrowserDetails, String> {
    let fields = std::str::from_utf8(log)
        .map_err(|_| "Git 提交信息不是 UTF-8")?
        .split('\0')
        .collect::<Vec<_>>();
    let field = |index: usize| {
        fields
            .get(index)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    Ok(GitBrowserDetails {
        repo_root: None,
        file: None,
        author: field(0),
        committed_at: field(1),
        subject: field(2),
        diff: String::from_utf8_lossy(diff).into_owned(),
    })
}

pub(crate) fn attach_git_file(
    details: &mut GitBrowserDetails,
    root: &str,
    status: &[u8],
) -> Result<(), String> {
    details.repo_root = Some(root.into());
    let mut files = git_directory(root, status, b"")?.files;
    if let Some(file) = files.first_mut() {
        if !details.diff.is_empty() && !details.diff.contains("Binary files ") {
            let mut in_hunk = false;
            let (mut added, mut removed) = (0, 0);
            for line in details.diff.lines() {
                if line.starts_with("diff --git ") {
                    in_hunk = false;
                } else if line.starts_with("@@") {
                    in_hunk = true;
                } else if in_hunk && line.starts_with('+') {
                    added += 1;
                } else if in_hunk && line.starts_with('-') {
                    removed += 1;
                }
            }
            file.additions = Some(added);
            file.deletions = Some(removed);
        }
    }
    details.file = files.into_iter().next();
    Ok(())
}

async fn local_git(cwd: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, String> {
    let mut command = vec![
        "--no-optional-locks".into(),
        "--literal-pathspecs".into(),
        "-c".into(),
        "core.quotepath=false".into(),
    ];
    command.extend(args.iter().map(|value| value.to_string()));
    let result = match crate::process::execute_process(
        PathBuf::from("git"),
        &command,
        cwd,
        30_000,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .await
    {
        Err(error) if error.contains("os error 2") => return Ok(None),
        result => result?,
    };
    if !result.status.success() {
        return Ok(None);
    }
    if result.stdout_capture_truncated {
        return Err("Git 输出过大，请查看更具体的子目录或文件".into());
    }
    Ok(Some(result.stdout))
}

pub async fn local_git_browser_directory(
    path: &str,
) -> Result<Option<GitBrowserDirectory>, String> {
    let cwd = std::fs::canonicalize(path).map_err(|error| format!("无法访问 Git 目录：{error}"))?;
    let Some(root) = local_git(&cwd, &["rev-parse", "--show-toplevel"]).await? else {
        return Ok(None);
    };
    let root = std::str::from_utf8(&root)
        .map_err(|_| "Git 根目录不是 UTF-8")?
        .trim();
    let status = local_git(
        &cwd,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--",
            ".",
        ],
    )
    .await?
    .ok_or("无法读取 Git 状态")?;
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--numstat",
        "-z",
        "HEAD",
        "--",
        ".",
    ];
    let counts = match local_git(&cwd, &args).await? {
        Some(value) => value,
        None => local_git(
            &cwd,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                "-z",
                "--",
                ".",
            ],
        )
        .await?
        .unwrap_or_default(),
    };
    git_directory(root, &status, &counts).map(Some)
}

pub async fn local_git_browser_details(path: &str) -> Result<GitBrowserDetails, String> {
    let file = std::fs::canonicalize(path).map_err(|error| format!("无法访问文件：{error}"))?;
    let cwd = file.parent().ok_or("文件没有父目录")?;
    let name = file
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("文件名不是 UTF-8")?;
    let Some(root) = local_git(cwd, &["rev-parse", "--show-toplevel"]).await? else {
        return git_details(b"", b"");
    };
    let root = std::str::from_utf8(&root)
        .map_err(|_| "Git 根目录不是 UTF-8")?
        .trim();
    let status = local_git(
        cwd,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--",
            name,
        ],
    )
    .await?
    .unwrap_or_default();
    let log = local_git(cwd, &["log", "-1", "--format=%an%x00%aI%x00%s", "--", name])
        .await?
        .unwrap_or_default();
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "HEAD",
        "--",
        name,
    ];
    let diff = match local_git(cwd, &args).await? {
        Some(value) => value,
        None => local_git(
            cwd,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--",
                name,
            ],
        )
        .await?
        .unwrap_or_default(),
    };
    let mut details = git_details(&log, &diff)?;
    attach_git_file(&mut details, root, &status)?;
    Ok(details)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_status_handles_renames_and_special_names() {
        let data = git_directory(
            "/repo",
            b" M a\tb\nc.rs\0R  new.rs\0old.rs\0?? fresh.rs\0",
            b"2\t1\ta\tb\nc.rs\01\t3\t\0old.rs\0new.rs\0",
        )
        .unwrap();
        assert_eq!(data.files[0].path, "/repo/a\tb\nc.rs");
        assert_eq!(data.files[0].additions, Some(2));
        assert_eq!(data.files[1].deletions, Some(3));
        assert_eq!(data.files[2].status, "??");
        assert!(git_directory("/repo", b"R  new.rs\0", b"").is_err());
        let details =
            git_details("小李\02026-10-12\0修改模块\n".as_bytes(), b"+line\n-line\n").unwrap();
        assert_eq!(details.author.as_deref(), Some("小李"));
    }
    #[test]
    fn current_file_counts_ignore_diff_headers_and_clear_clean_status() {
        let mut details = git_details(b"", b"diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1,2 @@\n-old\n+new\n+extra\n").unwrap();
        attach_git_file(&mut details, "/repo", b" M a.rs\0").unwrap();
        let file = details.file.as_ref().unwrap();
        assert_eq!(file.additions, Some(2));
        assert_eq!(file.deletions, Some(1));
        attach_git_file(&mut details, "/repo", b"").unwrap();
        assert!(details.file.is_none());
        assert_eq!(details.repo_root.as_deref(), Some("/repo"));
    }
}

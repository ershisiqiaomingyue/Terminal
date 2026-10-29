use super::*;
use crate::git_browser::{
    attach_git_file, git_details, git_directory, GitBrowserDetails, GitBrowserDirectory,
};

fn git_query(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    command: &str,
) -> Result<Option<Vec<u8>>, String> {
    // Exit failures (including no Git repository) are distinct from transport
    // failures. Encode each result to retain Git's NUL-separated paths.
    let script = format!("data=$({{ git --no-optional-locks --literal-pathspecs -c core.quotepath=false {command} || printf '\\0__GIT_ERROR__\\0'; }} | base64); printf '%s' \"$data\"");
    let result = run_ssh_workspace_command(
        config,
        password,
        root,
        &script,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(30),
    )?;
    if result.exit_code != 0 {
        return Ok(None);
    }
    if result.output_truncated {
        return Err("Git 输出过大，请查看更具体的子目录".into());
    }
    let encoded: String = result
        .stdout
        .chars()
        .filter(|value| !value.is_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("Git 数据解码失败：{error}"))?;
    if bytes.ends_with(b"\0__GIT_ERROR__\0") {
        return Ok(None);
    }
    if bytes.len() > 256 * 1024 {
        return Err("Git 输出超过 256 KB，请查看更具体的文件或子目录".into());
    }
    Ok(Some(bytes))
}

pub fn ssh_git_browser_directory(
    config: &SshConnectionConfig,
    password: Option<&str>,
    path: &str,
) -> Result<Option<GitBrowserDirectory>, String> {
    let fallback = is_openssh_transport_marked(config);
    let mut config = config.clone();
    config.id.push_str(":git-browser");
    if fallback {
        remember_openssh_transport(&config);
    }
    let config = &config;
    try_connection_operation(config, browser_lock_wait(config), || {
        let Some(root) = git_query(config, password, path, "rev-parse --show-toplevel")? else {
            return Ok(None);
        };
        let root = std::str::from_utf8(&root)
            .map_err(|_| "Git 目录不是 UTF-8")?
            .trim();
        let status = git_query(
            config,
            password,
            path,
            "status --porcelain=v1 -z --untracked-files=normal -- .",
        )?
        .ok_or("无法读取 Git 状态")?;
        let counts = match git_query(
            config,
            password,
            path,
            "diff --no-ext-diff --no-textconv --numstat -z HEAD -- .",
        )? {
            Some(value) => value,
            None => git_query(
                config,
                password,
                path,
                "diff --cached --no-ext-diff --no-textconv --numstat -z -- .",
            )?
            .unwrap_or_default(),
        };
        git_directory(root, &status, &counts).map(Some)
    })
}

pub fn ssh_git_browser_details(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    path: &str,
) -> Result<GitBrowserDetails, String> {
    if !path.starts_with('/') || path.split('/').any(|part| part == "..") {
        return Err("Git 文件路径无效".into());
    }
    let fallback = is_openssh_transport_marked(config);
    let mut config = config.clone();
    config.id.push_str(":git-browser");
    if fallback {
        remember_openssh_transport(&config);
    }
    let config = &config;
    try_connection_operation(config, browser_lock_wait(config), || {
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
            .unwrap_or(root);
        let root = parent;
        let Some(repo_root) = git_query(config, password, root, "rev-parse --show-toplevel")?
        else {
            return git_details(b"", b"");
        };
        let repo_root = std::str::from_utf8(&repo_root)
            .map_err(|_| "Git 根目录不是 UTF-8")?
            .trim();
        // The working directory follows directory links. A basename therefore
        // addresses the target repository correctly even through an alias.
        let path = shell_escape(path.rsplit('/').next().ok_or("Git 文件路径无效")?);
        let status = git_query(
            config,
            password,
            root,
            &format!("status --porcelain=v1 -z --untracked-files=normal -- {path}"),
        )?
        .unwrap_or_default();
        let log = git_query(
            config,
            password,
            root,
            &format!("log -1 --format='%an%x00%aI%x00%s' -- {path}"),
        )?
        .unwrap_or_default();
        let diff = match git_query(
            config,
            password,
            root,
            &format!("diff --no-ext-diff --no-textconv --no-color HEAD -- {path}"),
        )? {
            Some(value) => value,
            None => git_query(
                config,
                password,
                root,
                &format!("diff --cached --no-ext-diff --no-textconv --no-color -- {path}"),
            )?
            .unwrap_or_default(),
        };
        let mut details = git_details(&log, &diff)?;
        attach_git_file(&mut details, repo_root, &status)?;
        Ok(details)
    })
}

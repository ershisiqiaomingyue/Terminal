//! Explicit editor saves, separate from model-controlled workspace mutations.
use super::browser::{
    browser_connection, browser_file_guard, validate_browser_file_path, MAX_BROWSER_FILE_BYTES,
};
use super::*;
use std::sync::atomic::AtomicBool;

pub fn write_ssh_browser_file(
    config: &SshConnectionConfig,
    password: Option<&str>,
    root: &str,
    path: &str,
    content: &str,
    expected_version: &str,
) -> Result<String, String> {
    validate_browser_file_path(root, path)?;
    let expected_version = expected_version.trim_end_matches(['\r', '\n']);
    if content.len() > MAX_BROWSER_FILE_BYTES || content.contains('\0') {
        return Err("只能保存不超过 1 MB 的文本文件".into());
    }
    if expected_version.is_empty() {
        return Err("缺少文件版本，请重新加载后保存".into());
    }
    let config = browser_connection(config);
    // Stage in the destination directory and preserve metadata. The final
    // rename targets the resolved file, preserving the user's symbolic link.
    // Recheck the version after upload so a concurrent edit is not overwritten.
    let command = format!(
        "{guard}test -w \"$target\" || {{ printf '文件没有写入权限\\n' >&2; exit 1; }}; expected={expected}; check_version() {{ resolved=$(realpath -e -- {path} && printf '.') || return 1; resolved=${{resolved%.}}; resolved=${{resolved%?}}; [ \"$resolved\" = \"$target\" ] && [ \"$(stat -Lc '%d:%i:%s:%y:%z' -- \"$target\")\" = \"$expected\" ]; }}; check_version || {{ printf '远程文件已被其他人修改，未覆盖保存，请重新加载\\n' >&2; exit 1; }}; parent=${{target%/*}}; stage=$(mktemp -- \"${{parent:-/}}/.android-tool-save.XXXXXX\") || exit 1; trap 'rm -f -- \"$stage\"' EXIT; cp --attributes-only --preserve=all -- \"$target\" \"$stage\" || exit 1; base64 -d > \"$stage\" || exit 1; [ \"$(stat -c %s -- \"$stage\")\" = {length} ] || {{ printf '上传内容不完整，未保存\\n' >&2; exit 1; }}; check_version || {{ printf '远程文件已被其他人修改，未覆盖保存，请重新加载\\n' >&2; exit 1; }}; mv -fT -- \"$stage\" \"$target\" || exit 1; trap - EXIT; stat -Lc '%d:%i:%s:%y:%z' -- \"$target\"",
        guard = browser_file_guard(root, path), expected = shell_escape(expected_version), path = shell_escape(path), length = content.len(),
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
    let timeout = Duration::from_secs(config.operation_timeout_secs.clamp(10, 120));
    // 保存和浏览器的读/列目录/重命名走**同一条**命令通路
    // （run_ssh_workspace_command_with_input：russh 主传输 → ssh.exe → libssh2）。
    // 以前这里单独开一条裸 libssh2 会话，绕过了 russh 与统一错误处理：服务器
    // 只认现代算法时保存会卡住甚至失败，而同一个文件的读取却是正常的。
    try_connection_operation(&config, timeout, || {
        let result = run_ssh_workspace_command_with_input(
            &config,
            password,
            root,
            &command,
            Some(encoded.as_bytes()),
            Arc::new(AtomicBool::new(false)),
            timeout,
            None,
        )?;
        if result.exit_code != 0 {
            let message = result.stderr.trim();
            return Err(if message.is_empty() {
                "保存远程文件失败".to_string()
            } else {
                format!("保存远程文件失败：{message}")
            });
        }
        Ok(result.stdout.trim().to_string())
    })
}

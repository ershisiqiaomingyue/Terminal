use super::*;

pub(super) fn remote_text(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(content) => json!({ "content": content, "encoding": "utf-8" }),
        Err(_) => json!({
            "encoding": "hex",
            "contentHex": bytes.iter().take(8192).map(|byte| format!("{byte:02x}")).collect::<String>(),
            "binary": true,
        }),
    }
}

pub(super) fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn openssh_directory_copy_command(
    source: &str,
    destination: &str,
    overwrite: bool,
) -> String {
    let overwrite_guard = if overwrite { ":" } else { "exit 2" };
    format!(
        r#"if [ ! -d {source} ] || [ -L {source} ]; then exit 3; fi; \
if [ -e {destination} ] || [ -L {destination} ]; then \
  if [ ! -d {destination} ] || [ -L {destination} ]; then exit 4; fi; \
  {overwrite_guard}; \
  bad_destination=$(find {destination} -type l -print | sed -n '1p'); \
  if [ -n "$bad_destination" ]; then exit 10; fi; \
else \
  if [ ! -d {parent} ] || [ -L {parent} ]; then exit 5; fi; \
  if ! mkdir {destination}; then exit 6; fi; \
fi; \
bad_source=$(find {source} \( -type l -o ! \( -type f -o -type d \) \) -print | sed -n '1p'); \
if [ -n "$bad_source" ]; then exit 7; fi; \
entry_count=$(find {source} -print | awk 'END {{print NR - 1}}'); \
if [ "$entry_count" -gt {max_entries} ]; then exit 8; fi; \
byte_count=$(find {source} -type f -exec wc -c {{}} + | awk '{{total += $1}} END {{print total + 0}}'); \
if [ "$byte_count" -gt {max_bytes} ]; then exit 9; fi; \
if ! cp -pR {source}/. {destination}/; then exit 11; fi; \
printf '__AT_ENTRIES__%s\n__AT_BYTES__%s\n' "$entry_count" "$byte_count""#,
        source = shell_escape(source),
        destination = shell_escape(destination),
        parent = shell_escape(remote_parent_path(destination)),
        overwrite_guard = overwrite_guard,
        max_entries = MAX_REMOTE_DIRECTORY_ENTRIES,
        max_bytes = MAX_REMOTE_TRANSFER_BYTES,
    )
}

pub(super) fn openssh_directory_move_command(
    source: &str,
    destination: &str,
    overwrite: bool,
) -> String {
    let overwrite_guard = if overwrite { ":" } else { "exit 2" };
    format!(
        r#"if [ ! -d {source} ] || [ -L {source} ]; then exit 3; fi; \
if [ -e {destination} ] || [ -L {destination} ]; then \
  if [ ! -d {destination} ] || [ -L {destination} ]; then exit 4; fi; \
  {overwrite_guard}; \
  if ! rmdir {destination}; then exit 6; fi; \
else \
  if [ ! -d {parent} ] || [ -L {parent} ]; then exit 5; fi; \
fi; \
entry_count=$(find {source} -print | awk 'END {{print NR - 1}}'); \
if [ "$entry_count" -gt {max_entries} ]; then exit 8; fi; \
byte_count=$(find {source} -type f -exec wc -c {{}} + | awk '{{total += $1}} END {{print total + 0}}'); \
if [ "$byte_count" -gt {max_bytes} ]; then exit 9; fi; \
if ! mv {source} {destination}; then exit 11; fi; \
printf '__AT_ENTRIES__%s\n__AT_BYTES__%s\n' "$entry_count" "$byte_count""#,
        source = shell_escape(source),
        destination = shell_escape(destination),
        parent = shell_escape(remote_parent_path(destination)),
        overwrite_guard = overwrite_guard,
        max_entries = MAX_REMOTE_DIRECTORY_ENTRIES,
        max_bytes = MAX_REMOTE_TRANSFER_BYTES,
    )
}

pub(super) fn openssh_directory_listing_command(path: &str) -> String {
    format!(
        r#"if [ ! -d {path} ] || [ -L {path} ]; then exit 3; fi; \
bad_entry=$(find {path} \( -type l -o ! \( -type f -o -type d \) \) -print | sed -n '1p'); \
if [ -n "$bad_entry" ]; then exit 7; fi; \
entry_count=$(find {path} -print | awk 'END {{print NR - 1}}'); \
if [ "$entry_count" -gt {max_entries} ]; then exit 8; fi; \
byte_count=$(find {path} -type f -exec wc -c {{}} + | awk '{{total += $1}} END {{print total + 0}}'); \
if [ "$byte_count" -gt {max_bytes} ]; then exit 9; fi; \
printf '__AT_ENTRIES__%s\n__AT_BYTES__%s\n' "$entry_count" "$byte_count"; \
find {path} -type f -print"#,
        path = shell_escape(path),
        max_entries = MAX_REMOTE_DIRECTORY_ENTRIES,
        max_bytes = MAX_REMOTE_TRANSFER_BYTES,
    )
}

pub(super) fn openssh_directory_file_list(
    output: &Output,
    operation: &str,
) -> Result<(usize, u64, Vec<String>), String> {
    let (entries, bytes) = parse_openssh_directory_result(output, operation)?;
    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with("__AT_"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if files.len() > MAX_REMOTE_DIRECTORY_ENTRIES {
        return Err(format!(
            "{operation} 返回的文件数量超过 {} 项限制",
            MAX_REMOTE_DIRECTORY_ENTRIES
        ));
    }
    Ok((entries, bytes, files))
}

pub(super) fn remote_relative_path_from_base(base: &str, child: &str) -> Result<PathBuf, String> {
    let prefix = if base == "/" {
        "/".to_string()
    } else {
        format!("{}/", base.trim_end_matches('/'))
    };
    let relative = child
        .strip_prefix(&prefix)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("远程文件不在目录范围内：{child}"))?;
    let path = PathBuf::from(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("远程目录包含越界文件路径：{child}"));
    }
    Ok(path)
}

pub(super) fn ensure_openssh_remote_directory(
    context: &SshToolContext,
    path: &str,
    timeout: Duration,
) -> Result<(), String> {
    let command = format!(
        "if [ -L {path} ]; then exit 2; elif [ -e {path} ]; then if [ ! -d {path} ]; then exit 3; fi; elif ! mkdir -p {path}; then exit 4; fi",
        path = shell_escape(path)
    );
    let output = run_openssh_context_command_checked(
        context,
        &command,
        None,
        timeout,
        None,
        &[path],
        "创建远程目录",
    )?;
    if output.status.success() {
        return Ok(());
    }
    let message = match output.status.code() {
        Some(2) => "远程目录路径是符号链接".to_string(),
        Some(3) => "远程目录路径已被普通文件占用".to_string(),
        _ => String::from_utf8_lossy(&output.stderr).trim().to_string(),
    };
    Err(if message.is_empty() {
        format!("创建远程目录失败：{path}")
    } else {
        format!("创建远程目录失败：{message}")
    })
}

pub(super) fn parse_openssh_directory_result(
    output: &Output,
    operation: &str,
) -> Result<(usize, u64), String> {
    if !output.status.success() {
        let message = match output.status.code() {
            Some(2) => "远程目标目录已存在；如需覆盖请明确传入 overwrite=true".to_string(),
            Some(3) => format!("{operation} 源路径不是目录或是符号链接"),
            Some(4) => format!("{operation} 目标路径不是目录或是符号链接"),
            Some(5) => format!("{operation} 目标父目录不存在或是符号链接"),
            Some(6) => format!("{operation} 目标目录不为空，未执行覆盖"),
            Some(7) => "copy_directory 只支持普通文件和目录，不支持符号链接或特殊文件".to_string(),
            Some(10) => format!("{operation} 目标目录包含符号链接，为避免越界已停止"),
            Some(11) => format!("{operation} 的远程文件系统操作失败"),
            Some(8) => format!("{operation} 超过 {} 项限制", MAX_REMOTE_DIRECTORY_ENTRIES),
            Some(9) => format!(
                "{operation} 超过 {} MB 限制",
                MAX_REMOTE_TRANSFER_BYTES / (1024 * 1024)
            ),
            _ => String::from_utf8_lossy(&output.stderr).trim().to_string(),
        };
        return Err(if message.is_empty() {
            format!("{operation} 远程执行失败")
        } else {
            format!("{operation} 失败：{message}")
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = stdout
        .lines()
        .find_map(|line| line.strip_prefix("__AT_ENTRIES__"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .ok_or_else(|| format!("{operation} 未返回远程目录项数量"))?;
    let bytes = stdout
        .lines()
        .find_map(|line| line.strip_prefix("__AT_BYTES__"))
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or_else(|| format!("{operation} 未返回远程目录字节数"))?;
    Ok((entries, bytes))
}

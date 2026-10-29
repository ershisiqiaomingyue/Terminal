use super::super::context::{LocalToolContext, MAX_FILE_READ_BYTES};
use super::super::utilities::{cancelled, is_allowed_tool_path};
use crate::{resolve_selected_path, ToolDefinition, ToolHandler};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const MAX_BINARY_PREVIEW_BYTES: usize = 8 * 1024;
const FILE_WINDOW_BUFFER_BYTES: usize = 64 * 1024;

pub(in crate::local_tools) struct ReadLocalFileTool(pub(in crate::local_tools) LocalToolContext);

pub(in crate::local_tools) fn hex_preview(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub(in crate::local_tools) fn prefix_looks_like_text(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    }
}

pub(in crate::local_tools) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    retain: usize,
    line: &mut Vec<u8>,
) -> Result<Option<bool>, String> {
    line.clear();
    let mut overflowed = false;
    let mut saw_any = false;
    loop {
        let (content_len, has_newline, at_eof, copied) = {
            let available = reader
                .fill_buf()
                .map_err(|error| format!("读取文件失败: {error}"))?;
            if available.is_empty() {
                (0, false, true, 0)
            } else {
                let (content_len, has_newline) =
                    match available.iter().position(|byte| *byte == b'\n') {
                        Some(index) => (index, true),
                        None => (available.len(), false),
                    };
                let copied = content_len.min(retain.saturating_sub(line.len()));
                line.extend_from_slice(&available[..copied]);
                (content_len, has_newline, false, copied)
            }
        };
        if at_eof {
            return Ok(saw_any.then_some(overflowed));
        }
        saw_any = true;
        if copied < content_len {
            overflowed = true;
        }
        reader.consume(content_len + usize::from(has_newline));
        if has_newline {
            return Ok(Some(overflowed));
        }
    }
}

pub(in crate::local_tools) fn char_bounded_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(in crate::local_tools) fn trim_line_ending(line: &[u8]) -> &[u8] {
    let without_newline = line.strip_suffix(b"\n").unwrap_or(line);
    without_newline
        .strip_suffix(b"\r")
        .unwrap_or(without_newline)
}

struct LineWindow {
    content: String,
    start_line: usize,
    end_line: usize,
    total_lines: usize,
    lines_returned: usize,
    content_truncated: bool,
    note: Option<String>,
}

fn read_line_window(
    path: &Path,
    start_line: usize,
    end_line: Option<usize>,
    max_bytes: usize,
) -> Result<LineWindow, String> {
    let file = fs::File::open(path).map_err(|error| format!("无法打开文件: {error}"))?;
    let mut reader = BufReader::with_capacity(FILE_WINDOW_BUFFER_BYTES, file);
    let mut line = Vec::new();
    let mut content = String::new();
    let last_requested_line = end_line.unwrap_or(usize::MAX);
    let mut line_number = 0usize;
    let mut lines_returned = 0usize;
    let mut last_returned_line = 0usize;
    let mut content_truncated = false;
    loop {
        let overflowed = match read_bounded_line(&mut reader, max_bytes, &mut line)? {
            Some(overflowed) => overflowed,
            None => break,
        };
        line_number += 1;
        if line_number < start_line || line_number > last_requested_line || content_truncated {
            continue;
        }
        let text = String::from_utf8_lossy(trim_line_ending(&line)).into_owned();
        let separator = if lines_returned == 0 { "" } else { "\n" };
        let room = max_bytes.saturating_sub(content.len() + separator.len());
        if !overflowed && text.len() <= room {
            content.push_str(separator);
            content.push_str(&text);
            lines_returned += 1;
            last_returned_line = line_number;
            continue;
        }
        content_truncated = true;
        if content.len() + separator.len() >= max_bytes {
            continue;
        }
        content.push_str(separator);
        content.push_str(char_bounded_prefix(&text, room));
        lines_returned += 1;
        last_returned_line = line_number;
    }
    let returned_end_line = if lines_returned == 0 {
        start_line.saturating_sub(1)
    } else {
        last_returned_line
    };
    let note = if line_number == 0 {
        Some(format!(
            "文件为空（0 行），start_line={start_line} 没有可返回的行。"
        ))
    } else if start_line > line_number {
        Some(format!("start_line={start_line} 超过文件总行数 {line_number}：没有可返回的行，不要把前缀内容当作该窗口。"))
    } else if let Some(end) = end_line.filter(|end| *end < start_line) {
        Some(format!(
            "end_line={end} 小于 start_line={start_line}：没有可返回的行。"
        ))
    } else if content_truncated && lines_returned == 0 {
        Some(format!(
            "max_bytes={max_bytes} 太小，不足以返回第 {start_line} 行的任何内容。"
        ))
    } else if content_truncated {
        Some(format!("返回内容在 max_bytes={max_bytes} 处截断，最后一行（第 {last_returned_line} 行）只返回了开头部分；继续读取请把 start_line 设为 {}。", last_returned_line.saturating_add(1)))
    } else {
        None
    };
    Ok(LineWindow {
        content,
        start_line,
        end_line: returned_end_line,
        total_lines: line_number,
        lines_returned,
        content_truncated,
        note,
    })
}

#[async_trait]
impl ToolHandler for ReadLocalFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_local_file",
            "Read a bounded preview from one explicitly selected ordinary local file, or an explicitly requested external file when full access is enabled. Text files return UTF-8 content; binary files return a bounded hexadecimal preview. Do not pass a directory, archive, or archive member; use list_local_files or read_local_log_range instead. Pass start_line/end_line to read a window of real file lines instead of the first 64 KB: after a search told you where the match is, that is also how you read past the prefix.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "max_bytes": { "type": "integer", "maximum": MAX_FILE_READ_BYTES },
                    "start_line": { "type": "integer", "minimum": 1, "description": "1-based first line of the file to return, counted in real file lines and therefore not limited by the 64 KB prefix. Use the lineStart values returned by search_local_code to jump straight to a match instead of re-reading from the top." },
                    "end_line": { "type": "integer", "minimum": 1, "description": "1-based last line to return, inclusive. With start_line this turns a read into a bounded window; the returned text is still capped at max_bytes and the last line may be cut short." }
                },
                "required": ["path"]
            }),
        )
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let context = &self.0;
        cancelled(&context.cancellation)?;
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .map(|path| resolve_selected_path(&path, &context.roots))
            .ok_or_else(|| "read_local_file 需要 path".to_string())?;
        if !is_allowed_tool_path(
            &path,
            &context.roots,
            &context.excluded_paths,
            context.allow_external_paths,
        ) {
            return Err("只能读取用户明确选择的文件或目录".to_string());
        }
        let max_bytes = input
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(MAX_FILE_READ_BYTES as u64)
            .min(MAX_FILE_READ_BYTES as u64) as usize;
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "无法读取文件元数据：{}：{error}。请使用 list_local_files 返回的精确文件名，不要自行改写数字、空格或扩展名。",
                path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err("read_local_file 只支持文件".to_string());
        }
        let file = fs::File::open(&path).map_err(|error| format!("无法打开文件: {error}"))?;
        let mut bytes = Vec::with_capacity(max_bytes);
        file.take(max_bytes as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("读取文件失败: {error}"))?;
        let text_prefix = prefix_looks_like_text(&bytes);
        let binary_bytes = bytes.len().min(MAX_BINARY_PREVIEW_BYTES);
        let text = String::from_utf8_lossy(&bytes).to_string();
        let start_line = input
            .get("start_line")
            .and_then(Value::as_u64)
            .filter(|value| *value >= 1)
            .map(|value| value as usize);
        let end_line = input
            .get("end_line")
            .and_then(Value::as_u64)
            .filter(|value| *value >= 1)
            .map(|value| value as usize);
        let windowed = start_line.is_some() || end_line.is_some();
        let (content, window, note, truncated) = if windowed && prefix_looks_like_text(&bytes) {
            let window = read_line_window(&path, start_line.unwrap_or(1), end_line, max_bytes)?;
            let truncated = window.content_truncated;
            let value = json!({
                "startLine": window.start_line,
                "endLine": window.end_line,
                "totalLines": window.total_lines,
                "linesReturned": window.lines_returned,
                "contentTruncated": window.content_truncated,
            });
            (Some(window.content), Some(value), window.note, truncated)
        } else {
            (
                text_prefix.then(|| text.clone()),
                None,
                None,
                metadata.len() > bytes.len() as u64,
            )
        };
        let text_returned = content.is_some();
        let mut result = json!({
            "path": path.to_string_lossy(),
            "content": content,
            "encoding": if text_returned { "utf-8" } else { "hex" },
            "contentHex": (!text_returned).then(|| hex_preview(&bytes[..binary_bytes])),
            "binaryPreviewTruncated": !text_returned && bytes.len() > binary_bytes,
            "window": window,
            "truncated": truncated,
        });
        if let Some(note) = note {
            result["note"] = Value::String(note);
        }
        Ok(result)
    }
}

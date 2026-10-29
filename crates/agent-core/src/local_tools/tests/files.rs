use super::temporary_root;
use crate::local_tools::context::MAX_FILE_READ_BYTES;
use crate::local_tools::files::read::{
    char_bounded_prefix, read_bounded_line, trim_line_ending, ReadLocalFileTool,
};
use crate::LocalToolContext;
use crate::ToolHandler;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A line window must return exactly the requested lines by real file line
/// number, keep the default whole-prefix behaviour when omitted, and report
/// out-of-range input explicitly instead of clamping it.
#[tokio::test]
async fn read_local_file_returns_a_bounded_line_window() {
    let root = temporary_root();
    fs::create_dir_all(&root).unwrap();
    let path = root.join("window.txt");
    let body: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
    fs::write(&path, body.join("\n")).unwrap();

    let context = LocalToolContext::new(
        vec![root.to_string_lossy().into_owned()],
        vec![],
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    );

    // No window: the whole file, exactly as before.
    let full = ReadLocalFileTool(context.clone())
        .execute(json!({ "path": path.to_string_lossy() }))
        .await
        .unwrap();
    assert_eq!(full["window"], Value::Null);
    assert_eq!(full["content"].as_str().unwrap(), body.join("\n"));

    // Explicit window: only the requested lines.
    let window = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "start_line": 3,
            "end_line": 5
        }))
        .await
        .unwrap();
    assert_eq!(window["content"].as_str().unwrap(), "line3\nline4\nline5");
    assert_eq!(window["window"]["startLine"], 3);
    assert_eq!(window["window"]["endLine"], 5);
    assert_eq!(window["window"]["totalLines"], 10);

    // start_line alone runs to the end.
    let tail = ReadLocalFileTool(context.clone())
        .execute(json!({ "path": path.to_string_lossy(), "start_line": 9 }))
        .await
        .unwrap();
    assert_eq!(tail["content"].as_str().unwrap(), "line9\nline10");

    // A start_line past the end of the file is an explicit empty result,
    // never a silent fall back to the tail of the prefix.
    let beyond = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "start_line": 50,
            "end_line": 99
        }))
        .await
        .unwrap();
    assert_eq!(beyond["content"].as_str().unwrap(), "");
    assert_eq!(beyond["window"]["startLine"], 50);
    assert_eq!(beyond["window"]["endLine"], 49);
    assert_eq!(beyond["window"]["totalLines"], 10);
    assert_eq!(beyond["window"]["linesReturned"], 0);
    assert!(beyond["note"].as_str().unwrap().contains("超过文件总行数"));

    fs::remove_dir_all(root).unwrap();
}

/// The default read bound is 64 KB: a larger file is truncated, and an
/// explicit max_bytes request cannot raise the bound. Reading past it stays
/// possible through the start_line/end_line window, which addresses real
/// file lines (see `read_local_file_window_pages_real_file_lines`).
#[tokio::test]
async fn read_local_file_caps_the_default_prefix_at_sixty_four_kilobytes() {
    assert_eq!(MAX_FILE_READ_BYTES, 64 * 1024);
    // The shared temporary_root() uses %TEMP%, which is ACL-restricted in
    // some sandboxes; this fixture stays under the crate's own target
    // directory, which the test process is guaranteed to own.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("read-limit-fixture-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("large.txt");
    fs::write(&path, "a".repeat(MAX_FILE_READ_BYTES + 4 * 1024)).unwrap();

    let context = LocalToolContext::new(
        vec![root.to_string_lossy().into_owned()],
        vec![],
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    );

    let default_read = ReadLocalFileTool(context.clone())
        .execute(json!({ "path": path.to_string_lossy() }))
        .await
        .unwrap();
    assert_eq!(default_read["truncated"], true);
    assert_eq!(
        default_read["content"].as_str().unwrap().len(),
        MAX_FILE_READ_BYTES
    );

    // An oversized explicit request is clamped instead of honouring 10 MB.
    let clamped = ReadLocalFileTool(context)
        .execute(json!({
            "path": path.to_string_lossy(),
            "max_bytes": 10 * 1024 * 1024
        }))
        .await
        .unwrap();
    assert_eq!(clamped["truncated"], true);
    assert_eq!(
        clamped["content"].as_str().unwrap().len(),
        MAX_FILE_READ_BYTES
    );

    fs::remove_dir_all(root).unwrap();
}

/// A single line longer than the retention cap must not grow the buffer: the
/// reader drops the excess and consumes the rest of the line.
#[test]
fn bounded_line_reader_never_retains_more_than_the_cap() {
    let data = vec![b'x'; 4 * 1024 * 1024];
    let mut reader = std::io::BufReader::with_capacity(4 * 1024, std::io::Cursor::new(data));
    let mut line = Vec::new();
    let overflowed = read_bounded_line(&mut reader, 1024, &mut line)
        .unwrap()
        .unwrap();
    assert!(overflowed);
    assert_eq!(line.len(), 1024);
    // The whole 4 MB line was consumed, so the stream is at EOF.
    assert!(read_bounded_line(&mut reader, 1024, &mut line)
        .unwrap()
        .is_none());

    // Character-boundary trimming and line ending trimming.
    assert_eq!(char_bounded_prefix("中文", 4), "中");
    assert_eq!(char_bounded_prefix("中文", 5), "中");
    assert_eq!(char_bounded_prefix("中文", 6), "中文");
    assert_eq!(char_bounded_prefix("中文", 2), "");
    assert_eq!(trim_line_ending(b"line\r\n"), b"line");
    assert_eq!(trim_line_ending(b"line\n"), b"line");
    assert_eq!(trim_line_ending(b"line"), b"line");
}

/// The window addresses real file lines, so content far beyond the 64 KB
/// prefix stays readable; the default prefix read is unchanged.
#[tokio::test]
async fn read_local_file_window_pages_real_file_lines() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("read-window-fixture-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("deep.txt");
    let body: String = (1..=20_000)
        .map(|n| format!("line{n:05} marker{n:05}\n"))
        .collect();
    fs::write(&path, &body).unwrap();
    assert!(body.len() > 200 * 1024);

    let context = LocalToolContext::new(
        vec![root.to_string_lossy().into_owned()],
        vec![],
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    );

    // A deep window reads real lines far beyond the prefix, cheaply.
    let window = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "start_line": 15_000,
            "end_line": 15_010
        }))
        .await
        .unwrap();
    let content = window["content"].as_str().unwrap();
    assert!(content.starts_with("line15000 marker15000"));
    assert!(content.ends_with("line15010 marker15010"));
    assert!(!content.contains("line00001"));
    assert!(content.len() < 4 * 1024);
    assert_eq!(window["window"]["startLine"], 15_000);
    assert_eq!(window["window"]["endLine"], 15_010);
    assert_eq!(window["window"]["totalLines"], 20_000);
    assert_eq!(window["window"]["linesReturned"], 11);
    assert_eq!(window["window"]["contentTruncated"], false);
    assert_eq!(window["truncated"], false);
    assert!(window.get("note").is_none());

    // Without start_line/end_line the read is exactly the old 64 KB prefix.
    let default_read = ReadLocalFileTool(context.clone())
        .execute(json!({ "path": path.to_string_lossy() }))
        .await
        .unwrap();
    assert_eq!(default_read["window"], Value::Null);
    assert_eq!(default_read["truncated"], true);
    let default_content = default_read["content"].as_str().unwrap();
    assert_eq!(default_content.len(), MAX_FILE_READ_BYTES);
    assert!(default_content.starts_with("line00001 marker00001"));
    assert!(!default_content.contains("line15000"));

    // A start_line past the end is an explicit empty window, never a silent
    // fall back to the tail of the prefix.
    let beyond = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "start_line": 99_999
        }))
        .await
        .unwrap();
    assert_eq!(beyond["content"].as_str().unwrap(), "");
    assert_eq!(beyond["window"]["linesReturned"], 0);
    assert_eq!(beyond["window"]["endLine"], 99_998);
    assert_eq!(beyond["window"]["totalLines"], 20_000);
    assert!(beyond["note"].as_str().unwrap().contains("超过文件总行数"));

    // A window too large for max_bytes is cut on a character boundary and
    // says so; an explicit 10 MB request is still clamped to 64 KB.
    let cut = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "start_line": 1,
            "end_line": 20_000,
            "max_bytes": 10 * 1024 * 1024
        }))
        .await
        .unwrap();
    assert_eq!(cut["content"].as_str().unwrap().len(), MAX_FILE_READ_BYTES);
    assert_eq!(cut["truncated"], true);
    assert_eq!(cut["window"]["contentTruncated"], true);
    assert!(cut["window"]["endLine"].as_u64().unwrap() < 20_000);
    assert!(cut["note"].as_str().unwrap().contains("截断"));

    // CJK/emoji lines: the 64 KB prefix of this file stops inside a
    // multi-byte character, so strict UTF-8 validation rejects that prefix,
    // yet the window must still return real text lines without panicking.
    let cjk_path = root.join("cjk.txt");
    let mut cjk = "a".repeat(MAX_FILE_READ_BYTES - 1);
    cjk.push('中');
    for n in 1..=2_000 {
        cjk.push('\n');
        cjk.push_str(&format!("中文😀窗口行{n}"));
    }
    fs::write(&cjk_path, cjk.as_bytes()).unwrap();
    assert!(std::str::from_utf8(&cjk.as_bytes()[..MAX_FILE_READ_BYTES]).is_err());

    let cjk_window = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": cjk_path.to_string_lossy(),
            "start_line": 500,
            "end_line": 502
        }))
        .await
        .unwrap();
    assert_eq!(
        cjk_window["content"].as_str().unwrap(),
        "中文😀窗口行499\n中文😀窗口行500\n中文😀窗口行501"
    );
    assert_eq!(cjk_window["encoding"], "utf-8");
    assert_eq!(cjk_window["contentHex"], Value::Null);
    assert_eq!(cjk_window["window"]["totalLines"], 2_001);

    // The default (prefix) read of that same file is text too: a byte cap
    // that lands inside a character must not turn a readable text file into
    // a hexadecimal preview (that classification used to depend on where the
    // cap happened to fall). The cut character becomes U+FFFD and the result
    // still reports truncation.
    let cjk_default = ReadLocalFileTool(context.clone())
        .execute(json!({ "path": cjk_path.to_string_lossy() }))
        .await
        .unwrap();
    assert_eq!(cjk_default["encoding"], "utf-8");
    assert_eq!(cjk_default["contentHex"], Value::Null);
    assert_eq!(cjk_default["truncated"], true);
    assert_eq!(cjk_default["window"], Value::Null);
    let cjk_prefix = cjk_default["content"].as_str().unwrap();
    assert!(cjk_prefix.starts_with(&"a".repeat(1_000)));
    assert!(cjk_prefix.ends_with('\u{fffd}'));
    assert!(!cjk_prefix.contains("窗口行"));

    // end_line alone still starts at the first line.
    let head_window = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "end_line": 2
        }))
        .await
        .unwrap();
    assert_eq!(
        head_window["content"].as_str().unwrap(),
        "line00001 marker00001\nline00002 marker00002"
    );
    assert_eq!(head_window["window"]["endLine"], 2);

    // end_line before start_line is an explicit empty window.
    let inverted = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": path.to_string_lossy(),
            "start_line": 8,
            "end_line": 3
        }))
        .await
        .unwrap();
    assert_eq!(inverted["content"].as_str().unwrap(), "");
    assert_eq!(inverted["window"]["linesReturned"], 0);
    assert!(inverted["note"]
        .as_str()
        .unwrap()
        .contains("小于 start_line"));

    // An empty file has no lines to report.
    let empty_path = root.join("empty.txt");
    fs::write(&empty_path, "").unwrap();
    let empty = ReadLocalFileTool(context.clone())
        .execute(json!({
            "path": empty_path.to_string_lossy(),
            "start_line": 1
        }))
        .await
        .unwrap();
    assert_eq!(empty["content"].as_str().unwrap(), "");
    assert_eq!(empty["window"]["totalLines"], 0);
    assert!(empty["note"].as_str().unwrap().contains("文件为空"));

    // A binary file keeps its hexadecimal preview even when a window is
    // requested: the window path never rewrites binary handling.
    let binary_path = root.join("blob.bin");
    fs::write(&binary_path, [0x00_u8, 0xff, 0xfe, 0x80, 0x01]).unwrap();
    let binary = ReadLocalFileTool(context)
        .execute(json!({
            "path": binary_path.to_string_lossy(),
            "start_line": 1,
            "end_line": 3
        }))
        .await
        .unwrap();
    assert_eq!(binary["content"], Value::Null);
    assert_eq!(binary["encoding"], "hex");
    assert_eq!(binary["window"], Value::Null);
    assert_eq!(binary["contentHex"].as_str().unwrap(), "00fffe8001");

    fs::remove_dir_all(root).unwrap();
}

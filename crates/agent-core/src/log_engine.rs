use crate::{LogIndexEntry, LogRangeResult, LogSearchHit, LogTimelineEvent};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use zip::ZipArchive;

const MAX_INDEX_ENTRIES: usize = 4096;
const MAX_SCAN_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_RANGE_LINES: usize = 512;
const CANCELLATION_ERROR: &str = "请求已取消";

pub fn index_log_paths(paths: &[String]) -> Result<Vec<LogIndexEntry>, String> {
    let mut entries = Vec::new();
    for path in paths {
        let path = canonical_selected_path(path)?;
        let boundary = path.clone();
        collect_index_entries(&path, &boundary, &mut entries)?;
        if entries.len() >= MAX_INDEX_ENTRIES {
            break;
        }
    }
    Ok(entries)
}

pub fn search_log_paths(
    paths: &[String],
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
) -> Result<Vec<LogSearchHit>, String> {
    search_log_paths_impl(paths, query, max_results, max_excerpt_bytes, None)
}

pub fn search_log_paths_with_cancellation(
    paths: &[String],
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
    cancellation: &AtomicBool,
) -> Result<Vec<LogSearchHit>, String> {
    search_log_paths_impl(
        paths,
        query,
        max_results,
        max_excerpt_bytes,
        Some(cancellation),
    )
}

fn search_log_paths_impl(
    paths: &[String],
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
    cancellation: Option<&AtomicBool>,
) -> Result<Vec<LogSearchHit>, String> {
    check_cancellation(cancellation)?;
    let query = query.trim().to_lowercase();
    if query.is_empty() || max_results == 0 {
        return Ok(Vec::new());
    }
    let max_results = max_results.min(128);
    let max_excerpt_bytes = max_excerpt_bytes.clamp(64, 4096);
    let mut hits = Vec::new();
    for path in paths {
        check_cancellation(cancellation)?;
        let path = canonical_selected_path(path)?;
        let boundary = path.clone();
        search_path(
            &path,
            &boundary,
            &query,
            max_results,
            max_excerpt_bytes,
            &mut hits,
            cancellation,
        )?;
        if hits.len() >= max_results {
            break;
        }
    }
    check_cancellation(cancellation)?;
    Ok(hits)
}

pub fn read_log_range(
    paths: &[String],
    source: Option<&str>,
    member: Option<&str>,
    start_line: usize,
    max_lines: usize,
    max_bytes: usize,
) -> Result<LogRangeResult, String> {
    read_log_range_impl(
        paths, source, member, start_line, max_lines, max_bytes, None,
    )
}

pub fn read_log_range_with_cancellation(
    paths: &[String],
    source: Option<&str>,
    member: Option<&str>,
    start_line: usize,
    max_lines: usize,
    max_bytes: usize,
    cancellation: &AtomicBool,
) -> Result<LogRangeResult, String> {
    read_log_range_impl(
        paths,
        source,
        member,
        start_line,
        max_lines,
        max_bytes,
        Some(cancellation),
    )
}

fn read_log_range_impl(
    paths: &[String],
    source: Option<&str>,
    member: Option<&str>,
    start_line: usize,
    max_lines: usize,
    max_bytes: usize,
    cancellation: Option<&AtomicBool>,
) -> Result<LogRangeResult, String> {
    check_cancellation(cancellation)?;
    if paths.is_empty() {
        return Err("未选择日志来源".to_string());
    }
    let start_line = start_line.max(1);
    let max_lines = max_lines.clamp(1, MAX_RANGE_LINES);
    let max_bytes = max_bytes.clamp(256, 1024 * 1024);
    for path in paths {
        check_cancellation(cancellation)?;
        let path = canonical_selected_path(path)?;
        if let Some(result) = read_range_path(
            &path,
            source,
            member,
            start_line,
            max_lines,
            max_bytes,
            cancellation,
        )? {
            check_cancellation(cancellation)?;
            return Ok(result);
        }
    }
    check_cancellation(cancellation)?;
    Err("未找到指定的日志文件或压缩包成员".to_string())
}

pub fn rebuild_timeline(
    paths: &[String],
    max_events: usize,
) -> Result<Vec<LogTimelineEvent>, String> {
    rebuild_timeline_impl(paths, max_events, None)
}

pub fn rebuild_timeline_with_cancellation(
    paths: &[String],
    max_events: usize,
    cancellation: &AtomicBool,
) -> Result<Vec<LogTimelineEvent>, String> {
    rebuild_timeline_impl(paths, max_events, Some(cancellation))
}

fn rebuild_timeline_impl(
    paths: &[String],
    max_events: usize,
    cancellation: Option<&AtomicBool>,
) -> Result<Vec<LogTimelineEvent>, String> {
    check_cancellation(cancellation)?;
    let max_events = max_events.clamp(1, 512);
    let mut events = Vec::new();
    for path in paths {
        check_cancellation(cancellation)?;
        let path = canonical_selected_path(path)?;
        collect_timeline_path(&path, max_events, &mut events, cancellation)?;
        if events.len() >= max_events {
            break;
        }
    }
    events.truncate(max_events);
    check_cancellation(cancellation)?;
    Ok(events)
}

fn is_cancellation_requested(cancellation: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

fn check_cancellation(cancellation: Option<&AtomicBool>) -> Result<(), String> {
    if is_cancellation_requested(cancellation) {
        return Err(CANCELLATION_ERROR.to_string());
    }
    Ok(())
}

fn format_log_read_error(
    name: &str,
    error: io::Error,
    cancellation: Option<&AtomicBool>,
) -> String {
    if is_cancellation_requested(cancellation) {
        CANCELLATION_ERROR.to_string()
    } else {
        format!("无法读取日志 {name}: {error}")
    }
}

fn canonical_selected_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("无法访问用户选择的日志路径 {}: {error}", path.display()))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", canonical.display()))?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(format!("日志路径不是文件或目录: {}", canonical.display()));
    }
    Ok(canonical)
}

fn collect_index_entries(
    path: &Path,
    boundary: &Path,
    entries: &mut Vec<LogIndexEntry>,
) -> Result<(), String> {
    if entries.len() >= MAX_INDEX_ENTRIES {
        return Ok(());
    }
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Ok(());
    }
    let path = fs::canonicalize(path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    if !path.starts_with(boundary)
        || fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        return Ok(());
    }
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    if metadata.is_dir() {
        for child in fs::read_dir(&path)
            .map_err(|error| format!("无法读取日志目录 {}: {error}", path.display()))?
        {
            let child = child.map_err(|error| format!("无法读取日志目录项: {error}"))?;
            collect_index_entries(&child.path(), boundary, entries)?;
            if entries.len() >= MAX_INDEX_ENTRIES {
                break;
            }
        }
        return Ok(());
    }

    let source = path.to_string_lossy().to_string();
    if is_zip_path(&path) {
        let file = File::open(&path)
            .map_err(|error| format!("无法打开日志压缩包 {}: {error}", path.display()))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("无法读取日志压缩包 {}: {error}", path.display()))?;
        for index in 0..archive.len() {
            let entry = archive
                .by_index(index)
                .map_err(|error| format!("无法读取压缩包成员 {index}: {error}"))?;
            if entry.is_dir() {
                continue;
            }
            entries.push(LogIndexEntry {
                source: source.clone(),
                name: entry.name().to_string(),
                kind: "archive_member".to_string(),
                size: entry.size(),
                compressed_size: Some(entry.compressed_size()),
                is_archive: true,
            });
            if entries.len() >= MAX_INDEX_ENTRIES {
                break;
            }
        }
    } else {
        entries.push(LogIndexEntry {
            source,
            name: file_name(&path),
            kind: "file".to_string(),
            size: metadata.len(),
            compressed_size: None,
            is_archive: false,
        });
    }
    Ok(())
}

fn search_path(
    path: &Path,
    boundary: &Path,
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
    hits: &mut Vec<LogSearchHit>,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    check_cancellation(cancellation)?;
    if hits.len() >= max_results {
        return Ok(());
    }
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Ok(());
    }
    let path = fs::canonicalize(path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    if !path.starts_with(boundary)
        || fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        return Ok(());
    }
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    if metadata.is_dir() {
        for child in fs::read_dir(&path)
            .map_err(|error| format!("无法读取日志目录 {}: {error}", path.display()))?
        {
            let child = child.map_err(|error| format!("无法读取日志目录项: {error}"))?;
            search_path(
                &child.path(),
                boundary,
                query,
                max_results,
                max_excerpt_bytes,
                hits,
                cancellation,
            )?;
            if hits.len() >= max_results {
                break;
            }
        }
        return Ok(());
    }

    if is_zip_path(&path) {
        search_zip(
            &path,
            query,
            max_results,
            max_excerpt_bytes,
            hits,
            cancellation,
        )
    } else {
        search_text_file(
            &path,
            query,
            max_results,
            max_excerpt_bytes,
            hits,
            cancellation,
        )
    }
}

fn search_text_file(
    path: &Path,
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
    hits: &mut Vec<LogSearchHit>,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    check_cancellation(cancellation)?;
    let file = File::open(path)
        .map_err(|error| format!("无法打开日志文件 {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    search_reader(
        &mut reader,
        path.to_string_lossy().to_string(),
        file_name(path),
        query,
        max_results,
        max_excerpt_bytes,
        hits,
        cancellation,
    )
}

fn search_zip(
    path: &Path,
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
    hits: &mut Vec<LogSearchHit>,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    check_cancellation(cancellation)?;
    let file = File::open(path)
        .map_err(|error| format!("无法打开日志压缩包 {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("无法读取日志压缩包 {}: {error}", path.display()))?;
    check_cancellation(cancellation)?;
    for index in 0..archive.len() {
        check_cancellation(cancellation)?;
        if hits.len() >= max_results {
            break;
        }
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取压缩包成员 {index}: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut reader = BufReader::new(entry);
        search_reader(
            &mut reader,
            path.to_string_lossy().to_string(),
            name,
            query,
            max_results,
            max_excerpt_bytes,
            hits,
            cancellation,
        )?;
    }
    Ok(())
}

fn search_reader<R: BufRead>(
    reader: &mut R,
    source: String,
    name: String,
    query: &str,
    max_results: usize,
    max_excerpt_bytes: usize,
    hits: &mut Vec<LogSearchHit>,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    let mut line_number = 0usize;
    let mut byte_offset = 0u64;
    let mut scanned_bytes = 0usize;
    while let Some(line) = read_bounded_line(reader, cancellation)
        .map_err(|error| format_log_read_error(&name, error, cancellation))?
    {
        check_cancellation(cancellation)?;
        line_number += 1;
        scanned_bytes = scanned_bytes.saturating_add(line.byte_len);
        if line.text.to_lowercase().contains(query) {
            hits.push(LogSearchHit {
                source: source.clone(),
                name: name.clone(),
                line_start: line_number,
                line_end: line_number,
                byte_start: byte_offset,
                byte_end: byte_offset.saturating_add(line.byte_len as u64),
                excerpt: truncate_utf8(line.text.trim_end_matches(['\r', '\n']), max_excerpt_bytes),
            });
            if hits.len() >= max_results {
                break;
            }
        }
        byte_offset = byte_offset.saturating_add(line.byte_len as u64);
        if scanned_bytes >= MAX_SCAN_BYTES {
            break;
        }
    }
    Ok(())
}

fn read_range_path(
    path: &Path,
    source: Option<&str>,
    member: Option<&str>,
    start_line: usize,
    max_lines: usize,
    max_bytes: usize,
    cancellation: Option<&AtomicBool>,
) -> Result<Option<LogRangeResult>, String> {
    check_cancellation(cancellation)?;
    let path = fs::canonicalize(path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    if metadata.is_dir() {
        for child in fs::read_dir(&path)
            .map_err(|error| format!("无法读取日志目录 {}: {error}", path.display()))?
        {
            let child = child.map_err(|error| format!("无法读取日志目录项: {error}"))?;
            if let Some(result) = read_range_path(
                &child.path(),
                source,
                member,
                start_line,
                max_lines,
                max_bytes,
                cancellation,
            )? {
                return Ok(Some(result));
            }
        }
        return Ok(None);
    }
    if source.is_some_and(|source| source != path.to_string_lossy() && source != file_name(&path)) {
        return Ok(None);
    }
    if is_zip_path(&path) {
        let Some(member) = member else {
            return Ok(None);
        };
        let file = File::open(&path)
            .map_err(|error| format!("无法打开日志压缩包 {}: {error}", path.display()))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("无法读取日志压缩包 {}: {error}", path.display()))?;
        check_cancellation(cancellation)?;
        for index in 0..archive.len() {
            check_cancellation(cancellation)?;
            let entry = archive
                .by_index(index)
                .map_err(|error| format!("无法读取压缩包成员 {index}: {error}"))?;
            if entry.is_dir() || entry.name() != member {
                continue;
            }
            let mut reader = BufReader::new(entry);
            return read_range_reader(
                &mut reader,
                path.to_string_lossy().to_string(),
                member.to_string(),
                start_line,
                max_lines,
                max_bytes,
                cancellation,
            )
            .map(Some);
        }
        return Ok(None);
    }
    let file = File::open(&path)
        .map_err(|error| format!("无法打开日志文件 {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    read_range_reader(
        &mut reader,
        path.to_string_lossy().to_string(),
        file_name(&path),
        start_line,
        max_lines,
        max_bytes,
        cancellation,
    )
    .map(Some)
}

fn read_range_reader<R: BufRead>(
    reader: &mut R,
    source: String,
    name: String,
    start_line: usize,
    max_lines: usize,
    max_bytes: usize,
    cancellation: Option<&AtomicBool>,
) -> Result<LogRangeResult, String> {
    let mut line_number = 0usize;
    let mut byte_offset = 0u64;
    let mut content = String::new();
    let mut selected_start = 0u64;
    let mut selected_end = 0u64;
    let mut selected_lines = 0usize;
    let mut truncated = false;
    while let Some(line) = read_bounded_line(reader, cancellation)
        .map_err(|error| format_log_read_error(&name, error, cancellation))?
    {
        check_cancellation(cancellation)?;
        line_number += 1;
        let next_offset = byte_offset.saturating_add(line.byte_len as u64);
        if line_number >= start_line && selected_lines < max_lines {
            if selected_lines == 0 {
                selected_start = byte_offset;
            }
            let text = line.text.trim_end_matches(['\r', '\n']);
            let remaining = max_bytes.saturating_sub(content.len());
            if text.len() > remaining {
                content.push_str(&truncate_utf8(text, remaining));
                truncated = true;
            } else {
                content.push_str(text);
            }
            selected_end = next_offset;
            selected_lines += 1;
            if selected_lines >= max_lines || content.len() >= max_bytes || line.truncated {
                truncated |= line.truncated || content.len() >= max_bytes;
                break;
            }
            content.push('\n');
        }
        byte_offset = next_offset;
        if byte_offset as usize >= MAX_SCAN_BYTES {
            truncated = true;
            break;
        }
    }
    if selected_lines == 0 {
        return Err(format!("日志行范围超出文件内容: {name}"));
    }
    Ok(LogRangeResult {
        source,
        name,
        line_start: start_line,
        line_end: start_line + selected_lines - 1,
        byte_start: selected_start,
        byte_end: selected_end,
        content,
        truncated,
    })
}

fn collect_timeline_path(
    path: &Path,
    max_events: usize,
    events: &mut Vec<LogTimelineEvent>,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    check_cancellation(cancellation)?;
    if events.len() >= max_events {
        return Ok(());
    }
    let path = fs::canonicalize(path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("无法读取日志路径 {}: {error}", path.display()))?;
    if metadata.is_dir() {
        for child in fs::read_dir(&path)
            .map_err(|error| format!("无法读取日志目录 {}: {error}", path.display()))?
        {
            let child = child.map_err(|error| format!("无法读取日志目录项: {error}"))?;
            collect_timeline_path(&child.path(), max_events, events, cancellation)?;
            if events.len() >= max_events {
                break;
            }
        }
        return Ok(());
    }
    if is_zip_path(&path) {
        let file = File::open(&path)
            .map_err(|error| format!("无法打开日志压缩包 {}: {error}", path.display()))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("无法读取日志压缩包 {}: {error}", path.display()))?;
        check_cancellation(cancellation)?;
        for index in 0..archive.len() {
            check_cancellation(cancellation)?;
            if events.len() >= max_events {
                break;
            }
            let entry = archive
                .by_index(index)
                .map_err(|error| format!("无法读取压缩包成员 {index}: {error}"))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            let mut reader = BufReader::new(entry);
            collect_timeline_reader(
                &mut reader,
                path.to_string_lossy().to_string(),
                name,
                max_events,
                events,
                cancellation,
            )?;
        }
    } else {
        let file = File::open(&path)
            .map_err(|error| format!("无法打开日志文件 {}: {error}", path.display()))?;
        let mut reader = BufReader::new(file);
        collect_timeline_reader(
            &mut reader,
            path.to_string_lossy().to_string(),
            file_name(&path),
            max_events,
            events,
            cancellation,
        )?;
    }
    Ok(())
}

fn collect_timeline_reader<R: BufRead>(
    reader: &mut R,
    source: String,
    name: String,
    max_events: usize,
    events: &mut Vec<LogTimelineEvent>,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String> {
    let mut line = 0usize;
    let mut scanned_bytes = 0usize;
    while events.len() < max_events {
        check_cancellation(cancellation)?;
        let Some(record) = read_bounded_line(reader, cancellation)
            .map_err(|error| format_log_read_error(&name, error, cancellation))?
        else {
            break;
        };
        check_cancellation(cancellation)?;
        line += 1;
        scanned_bytes = scanned_bytes.saturating_add(record.byte_len);
        let summary = record.text.trim_end_matches(['\r', '\n']).trim();
        if !summary.is_empty() {
            events.push(LogTimelineEvent {
                source: source.clone(),
                name: name.clone(),
                line,
                timestamp: parse_timestamp(summary),
                summary: truncate_utf8(summary, 512),
            });
        }
        if scanned_bytes >= MAX_SCAN_BYTES {
            break;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct LineRecord {
    text: String,
    byte_len: usize,
    truncated: bool,
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    cancellation: Option<&AtomicBool>,
) -> io::Result<Option<LineRecord>> {
    let mut bytes = Vec::with_capacity(4096);
    let mut byte_len = 0usize;
    let mut truncated = false;
    loop {
        if is_cancellation_requested(cancellation) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                CANCELLATION_ERROR,
            ));
        }
        let available = reader.fill_buf()?;
        if is_cancellation_requested(cancellation) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                CANCELLATION_ERROR,
            ));
        }
        if available.is_empty() {
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        byte_len = byte_len.saturating_add(consumed);
        if bytes.len() < MAX_LINE_BYTES {
            let copy_len = (MAX_LINE_BYTES - bytes.len()).min(consumed);
            bytes.extend_from_slice(&available[..copy_len]);
            if copy_len < consumed {
                truncated = true;
            }
        } else {
            truncated = true;
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if byte_len == 0 {
        return Ok(None);
    }
    Ok(Some(LineRecord {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        byte_len,
        truncated,
    }))
}

fn parse_timestamp(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    for start in 0..bytes.len() {
        if start + 19 <= bytes.len()
            && bytes[start..start + 4].iter().all(u8::is_ascii_digit)
            && bytes[start + 4] == b'-'
            && bytes[start + 5..start + 7].iter().all(u8::is_ascii_digit)
            && bytes[start + 7] == b'-'
            && bytes[start + 8..start + 10].iter().all(u8::is_ascii_digit)
            && (bytes[start + 10] == b' ' || bytes[start + 10] == b'T')
            && bytes[start + 11..start + 13].iter().all(u8::is_ascii_digit)
            && bytes[start + 13] == b':'
            && bytes[start + 14..start + 16].iter().all(u8::is_ascii_digit)
            && bytes[start + 16] == b':'
            && bytes[start + 17..start + 19].iter().all(u8::is_ascii_digit)
        {
            return Some(line[start..start + 19].replace(' ', "T"));
        }
    }
    None
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

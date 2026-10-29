//! Bounded file snapshots and text differences shared by Agent hosts.
use crate::FileChangeEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_SNAPSHOT_FILES: usize = 2048;
const MAX_LINE_SCAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnapshot {
    pub path: String,
    pub size: u64,
    #[serde(rename = "modifiedAt")]
    pub modified_at: Option<u64>,
    pub hash: String,
    #[serde(rename = "lineCount")]
    pub line_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileChangeSummary {
    pub created: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    #[serde(rename = "addedLines")]
    pub added_lines: u64,
    #[serde(rename = "removedLines")]
    pub removed_lines: u64,
    #[serde(default)]
    pub entries: Vec<FileChangeEntry>,
}

pub fn capture_file_snapshot(paths: &[String]) -> Result<Vec<FileSnapshot>, String> {
    capture_file_snapshot_impl(paths, None, None)
}

pub fn capture_file_snapshot_with_cancellation(
    paths: &[String],
    cancellation: &AtomicBool,
) -> Result<Vec<FileSnapshot>, String> {
    capture_file_snapshot_impl(paths, Some(cancellation), None)
}

/// Reuse fingerprints for files whose size and precise modification time are
/// unchanged since the previous snapshot. This keeps end-of-turn file review
/// from rereading every file in a large workspace.
pub fn capture_file_snapshot_since(
    paths: &[String],
    previous: &[FileSnapshot],
    cancellation: &AtomicBool,
) -> Result<Vec<FileSnapshot>, String> {
    let previous = previous
        .iter()
        .map(|snapshot| (snapshot.path.clone(), snapshot))
        .collect::<HashMap<_, _>>();
    capture_file_snapshot_impl(paths, Some(cancellation), Some(&previous))
}

fn capture_file_snapshot_impl(
    paths: &[String],
    cancellation: Option<&AtomicBool>,
    previous: Option<&HashMap<String, &FileSnapshot>>,
) -> Result<Vec<FileSnapshot>, String> {
    let mut snapshots = Vec::new();
    for path in paths {
        check_cancellation(cancellation)?;
        match fs::metadata(Path::new(path)) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("无法读取文件变化路径 {path}: {error}")),
        }
        let canonical = canonicalize(path)?;
        collect_snapshot(
            &canonical,
            &canonical,
            &mut snapshots,
            cancellation,
            previous,
        )?;
        if snapshots.len() >= MAX_SNAPSHOT_FILES {
            break;
        }
    }
    Ok(snapshots)
}

const UNIFIED_DIFF_CONTEXT_LINES: usize = 3;
const MAX_UNIFIED_DIFF_OUTPUT_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy)]
enum DiffLine<'a> {
    Equal(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

impl DiffLine<'_> {
    fn is_equal(&self) -> bool {
        matches!(self, Self::Equal(_))
    }

    fn has_before(&self) -> bool {
        !matches!(self, Self::Added(_))
    }

    fn has_after(&self) -> bool {
        !matches!(self, Self::Removed(_))
    }

    fn text(&self) -> &str {
        match self {
            Self::Equal(text) | Self::Removed(text) | Self::Added(text) => text,
        }
    }

    fn marker(&self) -> char {
        match self {
            Self::Equal(_) => ' ',
            Self::Removed(_) => '-',
            Self::Added(_) => '+',
        }
    }
}

/// Produces a stable set of common-line anchors without an O(n*m) matrix.
/// Unique lines keep their original order through an LIS pass. The gaps are
/// emitted as remove/add blocks, which is always a valid unified diff even
/// for large generated files where a full LCS matrix would be unsafe.
fn anchored_line_ops<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<DiffLine<'a>> {
    let mut before_positions = HashMap::<&str, (usize, usize)>::new();
    let mut after_positions = HashMap::<&str, (usize, usize)>::new();
    for (index, line) in before.iter().enumerate() {
        let entry = before_positions.entry(*line).or_insert((index, 0));
        entry.1 += 1;
    }
    for (index, line) in after.iter().enumerate() {
        let entry = after_positions.entry(*line).or_insert((index, 0));
        entry.1 += 1;
    }
    let anchors = before
        .iter()
        .enumerate()
        .filter_map(|(before_index, line)| {
            let before_occurrence = before_positions.get(line)?;
            let after_occurrence = after_positions.get(line)?;
            (before_occurrence.1 == 1 && after_occurrence.1 == 1)
                .then_some((before_index, after_occurrence.0))
        })
        .collect::<Vec<_>>();

    let mut tails = Vec::<usize>::new();
    let mut previous = vec![None; anchors.len()];
    for (anchor_index, (_, after_index)) in anchors.iter().enumerate() {
        let mut low = 0usize;
        let mut high = tails.len();
        while low < high {
            let middle = (low + high) / 2;
            if anchors[tails[middle]].1 < *after_index {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if low > 0 {
            previous[anchor_index] = Some(tails[low - 1]);
        }
        if low == tails.len() {
            tails.push(anchor_index);
        } else {
            tails[low] = anchor_index;
        }
    }
    let mut ordered_anchors = Vec::with_capacity(tails.len());
    let mut cursor = tails.last().copied();
    while let Some(anchor_index) = cursor {
        ordered_anchors.push(anchors[anchor_index]);
        cursor = previous[anchor_index];
    }
    ordered_anchors.reverse();

    let mut before_cursor = 0usize;
    let mut after_cursor = 0usize;
    let mut operations = Vec::with_capacity(before.len() + after.len());
    for (before_index, after_index) in ordered_anchors {
        operations.extend(
            before[before_cursor..before_index]
                .iter()
                .copied()
                .map(DiffLine::Removed),
        );
        operations.extend(
            after[after_cursor..after_index]
                .iter()
                .copied()
                .map(DiffLine::Added),
        );
        operations.push(DiffLine::Equal(before[before_index]));
        before_cursor = before_index + 1;
        after_cursor = after_index + 1;
    }
    operations.extend(
        before[before_cursor..]
            .iter()
            .copied()
            .map(DiffLine::Removed),
    );
    operations.extend(after[after_cursor..].iter().copied().map(DiffLine::Added));
    operations
}

fn unified_range(start: usize, count: usize) -> String {
    if count == 0 {
        format!("{},0", start.saturating_sub(1))
    } else if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

/// Builds a bounded unified diff with common-line anchors and three context lines.
pub fn line_diff(before: &str, after: &str) -> String {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let operations = anchored_line_ops(&before_lines, &after_lines);
    let changed = operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| (!operation.is_equal()).then_some(index))
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return "没有文本差异".to_string();
    }

    let mut hunks = Vec::<(usize, usize)>::new();
    for changed_index in changed {
        let start = changed_index.saturating_sub(UNIFIED_DIFF_CONTEXT_LINES);
        let end = (changed_index + UNIFIED_DIFF_CONTEXT_LINES + 1).min(operations.len());
        if let Some((_, previous_end)) = hunks.last_mut() {
            if start <= *previous_end {
                *previous_end = (*previous_end).max(end);
                continue;
            }
        }
        hunks.push((start, end));
    }

    let mut output = String::from("--- 原始文件\n+++ 当前文件\n");
    for (start, end) in hunks {
        let before_start = operations[..start]
            .iter()
            .filter(|operation| operation.has_before())
            .count()
            + 1;
        let after_start = operations[..start]
            .iter()
            .filter(|operation| operation.has_after())
            .count()
            + 1;
        let before_count = operations[start..end]
            .iter()
            .filter(|operation| operation.has_before())
            .count();
        let after_count = operations[start..end]
            .iter()
            .filter(|operation| operation.has_after())
            .count();
        output.push_str(&format!(
            "@@ -{} +{} @@\n",
            unified_range(before_start, before_count),
            unified_range(after_start, after_count),
        ));
        for operation in &operations[start..end] {
            let line = format!("{}{}\n", operation.marker(), operation.text());
            if output.len() + line.len() > MAX_UNIFIED_DIFF_OUTPUT_BYTES {
                output.push_str("… 差异内容过长，已截断；请缩小审核范围。\n");
                return output;
            }
            output.push_str(&line);
        }
    }
    output
}

pub fn text_line_count(text: &str) -> u64 {
    text.lines().count() as u64
}

/// Counts the changed span after removing a common prefix and suffix.
pub fn text_line_changes(before: &str, after: &str) -> (u64, u64) {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let mut prefix = 0usize;
    while prefix < before_lines.len()
        && prefix < after_lines.len()
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }
    let mut before_end = before_lines.len();
    let mut after_end = after_lines.len();
    while before_end > prefix
        && after_end > prefix
        && before_lines[before_end - 1] == after_lines[after_end - 1]
    {
        before_end -= 1;
        after_end -= 1;
    }
    ((after_end - prefix) as u64, (before_end - prefix) as u64)
}

pub fn diff_file_snapshots(before: &[FileSnapshot], after: &[FileSnapshot]) -> FileChangeSummary {
    let before = before
        .iter()
        .map(|item| (item.path.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .iter()
        .map(|item| (item.path.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let mut diff = FileChangeSummary::default();
    for path in after.keys() {
        match before.get(path) {
            None => {
                diff.created.push(path.clone());
                diff.entries.push(FileChangeEntry {
                    path: path.clone(),
                    status: "created".to_string(),
                    added_lines: after[path].line_count.unwrap_or(0),
                    removed_lines: 0,
                });
            }
            Some(previous) if previous.hash != after[path].hash => {
                diff.modified.push(path.clone());
                let before_lines = previous.line_count.unwrap_or(0);
                let after_lines = after[path].line_count.unwrap_or(0);
                diff.entries.push(FileChangeEntry {
                    path: path.clone(),
                    status: "modified".to_string(),
                    added_lines: after_lines.saturating_sub(before_lines),
                    removed_lines: before_lines.saturating_sub(after_lines),
                });
            }
            Some(_) => {}
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            diff.deleted.push(path.clone());
            diff.entries.push(FileChangeEntry {
                path: path.clone(),
                status: "deleted".to_string(),
                added_lines: 0,
                removed_lines: before[path].line_count.unwrap_or(0),
            });
        }
    }
    diff.added_lines = diff.entries.iter().map(|entry| entry.added_lines).sum();
    diff.removed_lines = diff.entries.iter().map(|entry| entry.removed_lines).sum();
    diff
}

fn canonicalize(path: &str) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("无法读取文件变化路径 {path}: {error}"))
}

fn collect_snapshot(
    path: &Path,
    boundary: &Path,
    snapshots: &mut Vec<FileSnapshot>,
    cancellation: Option<&AtomicBool>,
    previous: Option<&HashMap<String, &FileSnapshot>>,
) -> Result<(), String> {
    check_cancellation(cancellation)?;
    if snapshots.len() >= MAX_SNAPSHOT_FILES {
        return Ok(());
    }
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Ok(());
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("无法读取文件变化路径 {}: {error}", path.display()))?;
    if !canonical.starts_with(boundary) {
        return Ok(());
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("无法读取文件变化路径 {}: {error}", canonical.display()))?;
    if metadata.is_dir() {
        let mut children = fs::read_dir(&canonical)
            .map_err(|error| format!("无法读取文件变化目录 {}: {error}", canonical.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort_by_key(|child| child.to_string_lossy().to_ascii_lowercase());
        for child in children {
            if should_skip_snapshot_directory(&child) {
                continue;
            }
            collect_snapshot(&child, boundary, snapshots, cancellation, previous)?;
            if snapshots.len() >= MAX_SNAPSHOT_FILES {
                break;
            }
        }
        return Ok(());
    }
    let path = display_path(&canonical);
    let modified_at = modified_timestamp(&metadata);
    if let Some(snapshot) = previous
        .and_then(|previous| previous.get(&path))
        .filter(|snapshot| snapshot.size == metadata.len() && snapshot.modified_at == modified_at)
    {
        snapshots.push((*snapshot).clone());
        return Ok(());
    }
    if metadata.len() > MAX_HASH_BYTES {
        snapshots.push(FileSnapshot {
            path,
            size: metadata.len(),
            modified_at,
            hash: format!("large:{}:{modified_at:?}", metadata.len()),
            line_count: None,
        });
        return Ok(());
    }
    let file = File::open(&canonical)
        .map_err(|error| format!("无法打开文件变化路径 {}: {error}", canonical.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut line_count = 0u64;
    let mut scanned = 0usize;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        check_cancellation(cancellation)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("无法读取文件变化路径 {}: {error}", canonical.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if scanned < MAX_LINE_SCAN_BYTES {
            let count_bytes = read.min(MAX_LINE_SCAN_BYTES - scanned);
            line_count = line_count.saturating_add(
                buffer[..count_bytes]
                    .iter()
                    .filter(|byte| **byte == b'\n')
                    .count() as u64,
            );
            scanned += count_bytes;
        }
    }
    snapshots.push(FileSnapshot {
        path,
        size: metadata.len(),
        modified_at,
        hash: format!("{:x}", hasher.finalize()),
        line_count: (metadata.len() as usize <= MAX_LINE_SCAN_BYTES).then_some(line_count),
    });
    Ok(())
}

fn check_cancellation(cancellation: Option<&AtomicBool>) -> Result<(), String> {
    if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err("请求已取消".to_string())
    } else {
        Ok(())
    }
}

fn modified_timestamp(metadata: &fs::Metadata) -> Option<u64> {
    metadata.modified().ok().and_then(|time| {
        time.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
    })
}

fn should_skip_snapshot_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    (matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".android-tool"
            | ".android-toolkit"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".gradle"
            | ".cache"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".next"
            | ".turbo"
            | "coverage"
    ) || (name.starts_with("target-") && path.join("debug/.fingerprint").is_dir()))
        && fs::metadata(path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
}

fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value.strip_prefix("\\\\?\\").unwrap_or(&value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_is_not_a_change_even_when_timestamp_changes() {
        let before = FileSnapshot {
            path: "source.rs".to_string(),
            size: 24,
            modified_at: Some(1),
            hash: "same-content".to_string(),
            line_count: Some(2),
        };
        let after = FileSnapshot {
            modified_at: Some(2),
            ..before.clone()
        };
        assert_eq!(
            diff_file_snapshots(&[before], &[after]),
            FileChangeSummary::default()
        );
    }

    #[test]
    fn same_line_count_with_different_content_is_still_a_real_change() {
        let before = FileSnapshot {
            path: "source.rs".to_string(),
            size: 24,
            modified_at: Some(1),
            hash: "old-content".to_string(),
            line_count: Some(2),
        };
        let after = FileSnapshot {
            hash: "new-content".to_string(),
            ..before.clone()
        };
        let diff = diff_file_snapshots(&[before], &[after]);
        assert_eq!(diff.modified, vec!["source.rs"]);
        assert_eq!(diff.entries.len(), 1);
    }

    #[test]
    fn text_diff_retains_context_and_handles_insertions_and_deletions() {
        assert_eq!(line_diff("one\ntwo\n", "one\ntwo\n"), "没有文本差异");
        let diff = line_diff("one\ntwo\nthree\n", "one\nchanged\nthree\n");
        assert!(diff.contains("@@ -1,3 +1,3 @@\n one\n-two\n+changed\n three\n"));
        assert!(line_diff("", "new\n").contains("@@ -0,0 +1 @@\n+new\n"));
        assert!(line_diff("old\n", "").contains("@@ -1 +0,0 @@\n-old\n"));
        assert_eq!(
            text_line_changes("one\ntwo\nthree\n", "one\nchanged\nthree\n"),
            (1, 1)
        );
    }

    #[test]
    fn snapshots_skip_build_output_and_track_created_modified_deleted_files() {
        let directory = std::env::temp_dir().join(format!(
            "agent-file-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(directory.join("target"))
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        fs::write(directory.join("target/ignored.rs"), "ignored\n").unwrap();
        fs::create_dir_all(directory.join(".cache/windows-tools")).unwrap();
        for index in 0..=MAX_SNAPSHOT_FILES {
            fs::write(
                directory.join(format!(".cache/windows-tools/{index}.bin")),
                "cached runtime\n",
            )
            .unwrap();
        }
        fs::create_dir_all(directory.join("target-check/debug/.fingerprint")).unwrap();
        fs::write(
            directory.join("target-check/debug/generated.bin"),
            "build artifact\n",
        )
        .unwrap();
        fs::write(directory.join("unchanged.rs"), "stable\n").unwrap();
        fs::write(directory.join("modified.rs"), "before\n").unwrap();
        fs::write(directory.join("deleted.rs"), "deleted\n").unwrap();
        let paths = vec![directory.to_string_lossy().into_owned()];
        let before = capture_file_snapshot(&paths).unwrap();
        assert_eq!(before.len(), 3);
        fs::write(directory.join("modified.rs"), "after\n").unwrap();
        fs::remove_file(directory.join("deleted.rs")).unwrap();
        fs::write(directory.join("created.rs"), "created\n").unwrap();
        let after = capture_file_snapshot_since(&paths, &before, &AtomicBool::new(false)).unwrap();
        let unchanged_before = before
            .iter()
            .find(|item| item.path.ends_with("unchanged.rs"))
            .unwrap();
        let unchanged_after = after
            .iter()
            .find(|item| item.path.ends_with("unchanged.rs"))
            .unwrap();
        assert_eq!(unchanged_after, unchanged_before);
        let changes = diff_file_snapshots(&before, &after);
        assert_eq!(
            (
                changes.created.len(),
                changes.modified.len(),
                changes.deleted.len()
            ),
            (1, 1, 1)
        );
        assert_eq!((changes.added_lines, changes.removed_lines), (1, 1));
        assert!(capture_file_snapshot_with_cancellation(&paths, &AtomicBool::new(true)).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}

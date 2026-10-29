use super::super::context::{LocalToolContext, MAX_SEARCH_FILE_BYTES, MAX_SEARCH_RESULTS};
use super::super::utilities::{cancelled, is_excluded_path, skip_agent_directory};
use super::spec::SearchSpec;
use crate::ToolDefinition;
use globset::GlobSet;
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub(super) fn definition(code: bool) -> ToolDefinition {
    ToolDefinition::new(
        if code { "search_local_code" } else { "search_local_files" },
        if code {
            "Search text in all source languages within the selected workspace. Literal by default; supports regex, include globs, and match-offset pagination. Skips dependencies/build caches, binary and oversized files. Check partial/skippedFiles before concluding no match; narrow path if the time budget expires."
        } else {
            "Search file names across the selected workspace, without a candidate-file limit. Supports regex, include globs and match-offset pagination. Check partial before concluding no match."
        },
        json!({"type":"object","properties":{
            "query":{"type":"string"}, "path":{"type":"string"},
            "max_results":{"type":"integer","minimum":1,"maximum":MAX_SEARCH_RESULTS},
            "case_sensitive":{"type":"boolean"}, "regex":{"type":"boolean"},
            "include":{"type":"array","items":{"type":"string"},"description":"Workspace-relative slash-separated globs, e.g. **/*.{rs,cpp,py,tsx}; omitted searches all text files."},
            "offset":{"type":"integer","minimum":0,"description":"Matching results to skip; use nextOffset for the next page. Stable while files are unchanged."},
            "files_only":{"type":"boolean","description":"Return one entry per matching file instead of one per matching line. Cheapest first probe when you only need to know where a symbol lives, before reading a line window with read_local_file."}
        },"required":["query"]}),
    ).read_only()
}

/// Resolved once per process. `None` means ripgrep is not usable here.
fn ripgrep_path() -> Option<&'static PathBuf> {
    static RESOLVED: OnceLock<Option<PathBuf>> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            // An explicit override wins, so tests and packaged builds can point
            // at a bundled binary.
            if let Some(value) = std::env::var_os("ANDROID_TOOL_RIPGREP") {
                let path = PathBuf::from(value);
                if probe_ripgrep(&path) {
                    return Some(path);
                }
            }
            let names: &[&str] = if cfg!(windows) {
                &["rg.exe", "rg"]
            } else {
                &["rg"]
            };
            if let Some(path) = std::env::var_os("PATH") {
                for directory in std::env::split_paths(&path) {
                    for name in names {
                        let candidate = directory.join(name);
                        if probe_ripgrep(&candidate) {
                            return Some(candidate);
                        }
                    }
                }
            }
            // winget's shim directory is frequently absent from a service PATH.
            if cfg!(windows) {
                if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                    for name in names {
                        let candidate = PathBuf::from(&local)
                            .join("Microsoft/WinGet/Links")
                            .join(name);
                        if probe_ripgrep(&candidate) {
                            return Some(candidate);
                        }
                    }
                }
            }
            None
        })
        .as_ref()
}

/// A wrong `rg` (wrong arch, non-executable shim) must fall through to the
/// scanner instead of failing the tool, so the probe runs the binary once.
fn probe_ripgrep(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    // ripgrep is a console program: without CREATE_NO_WINDOW its probe pops a
    // console window in the GUI host (see process::background_command).
    crate::process::background_command(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(super) struct Scan<'a> {
    pub(super) context: &'a LocalToolContext,
    pub(super) root: PathBuf,
    pub(super) boundary: PathBuf,
    pub(super) matcher: Regex,
    pub(super) include: Option<GlobSet>,
    pub(super) code: bool,
    /// Report each matching file once instead of every matching line.
    pub(super) files_only: bool,
    pub(super) offset: usize,
    pub(super) limit: usize,
    pub(super) matched: usize,
    pub(super) visited: usize,
    pub(super) skipped: usize,
    pub(super) results: Vec<Value>,
    pub(super) started: Instant,
    pub(super) timed_out: bool,
    pub(super) has_more: bool,
}

impl Scan<'_> {
    fn stopped(&mut self) -> Result<bool, String> {
        cancelled(&self.context.cancellation)?;
        self.timed_out |= self.started.elapsed() >= Duration::from_secs(20);
        Ok(self.timed_out || self.has_more)
    }

    fn add(&mut self, value: Value) {
        self.matched += 1;
        if self.matched <= self.offset {
            return;
        }
        if self.results.len() == self.limit {
            self.has_more = true;
        } else {
            self.results.push(value);
        }
    }

    pub(super) fn visit(&mut self, path: &Path) -> Result<(), String> {
        if self.stopped()? || is_excluded_path(path, &self.context.excluded_paths) {
            return Ok(());
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(_) => {
                self.skipped += 1;
                return Ok(());
            }
        };
        if metadata.file_type().is_symlink() {
            return Ok(());
        }
        let canonical = match fs::canonicalize(path) {
            Ok(value) if value.starts_with(&self.boundary) => value,
            _ => {
                self.skipped += 1;
                return Ok(());
            }
        };
        if metadata.is_dir() {
            // An explicitly selected generated directory can still be searched.
            if path != self.root && skip_agent_directory(path) {
                return Ok(());
            }
            let entries = match fs::read_dir(path) {
                Ok(entries) => entries,
                Err(_) => {
                    self.skipped += 1;
                    return Ok(());
                }
            };
            let mut children = Vec::new();
            for entry in entries {
                if self.stopped()? {
                    break;
                }
                match entry {
                    Ok(entry) => children.push(entry.path()),
                    Err(_) => self.skipped += 1,
                }
            }
            children.sort_by_key(|child| child.to_string_lossy().to_lowercase());
            for child in children {
                if self.stopped()? {
                    break;
                }
                self.visit(&child)?;
            }
        } else if metadata.is_file() {
            self.visited += 1;
            let relative_path = path
                .strip_prefix(&self.root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(path.file_name().unwrap_or_default()));
            let relative = relative_path.to_string_lossy().replace('\\', "/");
            if self
                .include
                .as_ref()
                .is_some_and(|set| !set.is_match(&relative))
            {
                return Ok(());
            }
            if !self.code {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if self.matcher.is_match(&name) {
                    self.add(json!({"name":name,"path":canonical.to_string_lossy(),"kind":"file"}));
                }
                return Ok(());
            }
            if metadata.len() > MAX_SEARCH_FILE_BYTES as u64 {
                self.skipped += 1;
                return Ok(());
            }
            // Bound the read as well as checking metadata: a file may grow during a scan.
            let mut bytes = Vec::new();
            let read = fs::File::open(path).and_then(|file| {
                file.take((MAX_SEARCH_FILE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
            });
            if read.is_err() || bytes.len() > MAX_SEARCH_FILE_BYTES || bytes.contains(&0) {
                self.skipped += 1;
                return Ok(());
            }
            let Ok(content) = std::str::from_utf8(&bytes) else {
                self.skipped += 1;
                return Ok(());
            };
            for (index, line) in content.lines().enumerate() {
                if self.stopped()? {
                    break;
                }
                if !self.matcher.is_match(line) {
                    continue;
                }
                // files_only: one entry per file, matching the ripgrep
                // --files-with-matches shape so both backends agree.
                if self.files_only {
                    let relative = path
                        .strip_prefix(&self.root)
                        .map(|value| value.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        });
                    self.add(json!({
                        "path": canonical.to_string_lossy(),
                        "relativePath": relative,
                        "kind": "file",
                    }));
                    break;
                }
                self.add(json!({"path":canonical.to_string_lossy(),"lineStart":index+1,"lineEnd":index+1,"excerpt":line.chars().take(400).collect::<String>()}));
            }
        }
        Ok(())
    }
}

/// Shared argument parsing so both backends agree on semantics.
/// Directories that the in-process scanner never descends into. ripgrep gets
/// the same list so both backends report the same result set.
const PRUNED_DIRECTORIES: &[&str] = &[
    ".git",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
    "target",
    "dist",
    "build",
    ".gradle",
];

fn ripgrep_glob_arguments(spec: &SearchSpec) -> Vec<String> {
    let mut args = Vec::new();
    for directory in PRUNED_DIRECTORIES {
        args.push("--glob".to_string());
        args.push(format!("!**/{directory}/**"));
    }
    // Symlinks are skipped by the scanner, so keep ripgrep in step.
    args.push("--no-follow".to_string());
    args.push("--max-filesize".to_string());
    args.push(MAX_SEARCH_FILE_BYTES.to_string());
    if !spec.include.is_empty() {
        for pattern in &spec.include {
            args.push("--glob".to_string());
            args.push(pattern.clone());
        }
    }
    args
}

/// Run ripgrep and translate its JSON stream into the tool's result shape.
/// Returns `None` when ripgrep cannot be used, so the caller falls back.
pub(super) fn run_ripgrep(
    spec: &SearchSpec,
    context: &LocalToolContext,
) -> Option<(Vec<Value>, usize, bool)> {
    let binary = ripgrep_path()?;
    // Same reason as probe_ripgrep: a bare Command would flash a console window
    // on every search tool call in the GUI host.
    let mut command = crate::process::background_command(binary);
    command
        .current_dir(&spec.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if spec.code {
        // text mode: report one JSON object per matching line
        command
            .arg("--json")
            .arg("--no-heading")
            .arg("--with-filename")
            .arg("--line-number")
            .arg("--color=never");
    } else {
        // files mode: only file paths, filtered by the query on the basename
        command.arg("--files").arg("--color=never");
    }
    command.arg("--no-messages");
    command.arg(if spec.case_insensitive {
        "--ignore-case"
    } else {
        "--case-sensitive"
    });
    if spec.code {
        command.arg("--regexp").arg(&spec.pattern);
        // One path per matching file: the cheapest answer when the caller only
        // needs to know where the symbol lives, not every matching line.
        if spec.files_only {
            command.arg("--files-with-matches");
        }
    }
    for argument in ripgrep_glob_arguments(spec) {
        command.arg(argument);
    }
    // `--files` cannot filter by name itself; the caller matches names instead.
    let output = command.output().ok()?;
    if !output.status.success() && output.status.code() != Some(1) {
        // 1 means "no matches" and is still a usable answer.
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let mut matched = 0usize;
    let mut has_more = false;
    let mut visited = 0usize;

    if spec.code && spec.files_only {
        // Plain path list, not the JSON event stream.
        for raw_line in text.lines() {
            let raw_line = raw_line.trim();
            if raw_line.is_empty() {
                continue;
            }
            matched += 1;
            if matched <= spec.offset {
                continue;
            }
            if results.len() == spec.limit {
                has_more = true;
                continue;
            }
            let absolute = spec.root.join(raw_line);
            let canonical = fs::canonicalize(&absolute).unwrap_or(absolute);
            visited += 1;
            results.push(json!({
                "path": canonical.to_string_lossy(),
                "relativePath": raw_line.replace('\\', "/"),
                "kind": "file",
            }));
        }
        return Some((results, visited, has_more));
    }

    if spec.code {
        for raw in text.lines() {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            if event["type"] == "end" {
                if let Some(searched) = event["data"]["stats"]["searches"].as_u64() {
                    visited = visited.max(searched as usize);
                }
                continue;
            }
            if event["type"] != "match" {
                continue;
            }
            let Some(data) = event.get("data") else {
                continue;
            };
            let Some(path) = data["path"]["text"].as_str() else {
                continue;
            };
            let absolute = spec.root.join(path);
            let canonical = match fs::canonicalize(&absolute) {
                Ok(value) if value.starts_with(&spec.boundary) => value,
                _ => continue,
            };
            if is_excluded_path(&canonical, &context.excluded_paths) {
                continue;
            }
            let line = data["line_number"].as_u64().unwrap_or(0);
            let excerpt = data["lines"]["text"]
                .as_str()
                .unwrap_or_default()
                .trim_end_matches(['\n', '\r'])
                .chars()
                .take(400)
                .collect::<String>();
            matched += 1;
            if matched <= spec.offset {
                continue;
            }
            if results.len() == spec.limit {
                has_more = true;
                break;
            }
            results.push(json!({
                "path": canonical.to_string_lossy(),
                "lineStart": line,
                "lineEnd": line,
                "excerpt": excerpt,
            }));
        }
    } else {
        let matcher = RegexBuilder::new(&spec.pattern)
            .case_insensitive(spec.case_insensitive)
            .size_limit(2 * 1024 * 1024)
            .build()
            .ok()?;
        for raw in text.lines() {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let absolute = spec.root.join(raw);
            let canonical = match fs::canonicalize(&absolute) {
                Ok(value) if value.starts_with(&spec.boundary) => value,
                _ => continue,
            };
            if is_excluded_path(&canonical, &context.excluded_paths) {
                continue;
            }
            visited += 1;
            let name = canonical
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            if !matcher.is_match(&name) {
                continue;
            }
            matched += 1;
            if matched <= spec.offset {
                continue;
            }
            if results.len() == spec.limit {
                has_more = true;
                break;
            }
            results.push(json!({
                "name": name,
                "path": canonical.to_string_lossy(),
                "kind": "file",
            }));
        }
    }
    Some((results, visited, has_more))
}

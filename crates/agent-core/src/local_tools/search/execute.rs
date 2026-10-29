use super::super::context::LocalToolContext;
use super::super::utilities::cancelled;
use super::scanner::{run_ripgrep, Scan};
use super::spec::parse_spec;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::time::Instant;

pub(in crate::local_tools) fn execute(
    context: &LocalToolContext,
    input: Value,
    code: bool,
) -> Result<Value, String> {
    let spec = parse_spec(context, &input, code)?;
    cancelled(&context.cancellation)?;

    // ripgrep cannot traverse the `\\wsl.localhost` UNC share reliably, so the
    // WSL target keeps the in-process scanner.
    if context.wsl.is_none() {
        if let Some((results, visited, has_more)) = run_ripgrep(&spec, context) {
            let next = has_more.then(|| spec.offset + results.len());
            return Ok(json!({
                "path": spec.root.to_string_lossy(),
                "query": spec.query,
                "results": results,
                "truncated": has_more,
                "partial": false,
                "nextOffset": next,
                "visitedFiles": visited,
                "skippedFiles": 0,
                "timedOut": false,
                "backend": "ripgrep",
                "scope": "Selected path; nested dependency/build/cache directories and symlinks excluded. All text file extensions searched.",
            }));
        }
    }

    let mut scan = Scan {
        context,
        root: spec.root.clone(),
        boundary: spec.boundary.clone(),
        matcher: RegexBuilder::new(&spec.pattern)
            .case_insensitive(spec.case_insensitive)
            .size_limit(2 * 1024 * 1024)
            .build()
            .map_err(|error| format!("检索表达式无效：{error}"))?,
        include: build_glob_set(&spec.include)?,
        code,
        files_only: spec.files_only,
        offset: spec.offset,
        limit: spec.limit,
        matched: 0,
        visited: 0,
        skipped: 0,
        results: Vec::new(),
        started: Instant::now(),
        timed_out: false,
        has_more: false,
    };
    scan.visit(&spec.root)?;
    Ok(
        json!({"path":spec.root.to_string_lossy(),"query":spec.query,"results":scan.results,
        "truncated":scan.has_more || scan.timed_out,"partial":scan.timed_out || scan.skipped>0,
        "nextOffset":if scan.has_more { Some(scan.offset+scan.results.len()) } else { None },
        "visitedFiles":scan.visited,"skippedFiles":scan.skipped,"timedOut":scan.timed_out,
        "backend":"scanner",
        "scope":"Selected path; nested dependency/build/cache directories and symlinks excluded. All text file extensions searched."}),
    )
}

pub(super) fn build_glob_set(patterns: &[String]) -> Result<Option<GlobSet>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|e| format!("include glob 无效：{e}"))?,
        );
    }
    Ok(Some(builder.build().map_err(|e| e.to_string())?))
}

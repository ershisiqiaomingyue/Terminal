use super::temporary_root;
use crate::local_tools::search::execute::execute;
use crate::LocalToolContext;
use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn searches_beyond_256_candidates_in_all_languages_and_paginates_matches() {
    let root = temporary_root();
    fs::create_dir_all(root.join(".cache")).unwrap();
    fs::create_dir_all(root.join("a-docs")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    for index in 0..300 {
        fs::write(root.join(format!("a-docs/{index:04}.txt")), "irrelevant").unwrap();
        fs::write(root.join(format!(".cache/{index:04}.py")), "needle42").unwrap();
    }
    for ext in ["rs", "cpp", "py", "tsx", "js", "go", "unusual"] {
        fs::write(root.join(format!("src/needle.{ext}")), "needle42\nneedle43").unwrap();
    }
    let context = LocalToolContext::new(
        vec![root.to_string_lossy().into_owned()],
        vec![],
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    );
    let first = execute(&context,json!({"query":"needle4[23]","regex":true,"include":["**/*.{rs,cpp,py,tsx}"],"max_results":3}),true).unwrap();
    assert_eq!(first["results"].as_array().unwrap().len(), 3);
    assert_eq!(first["nextOffset"], 3);
    // visitedFiles reports how much scope was searched. ripgrep prunes by
    // its own glob engine and only reports files it opened, so the
    // everything-was-visited guarantee only holds for the scanner.
    if first["backend"] == "scanner" {
        assert!(first["visitedFiles"].as_u64().unwrap() > 256);
    }
    let next = execute(
        &context,
        json!({"query":"needle4[23]","regex":true,"include":["**/*.{rs,cpp,py,tsx}"],"offset":3}),
        true,
    )
    .unwrap();
    assert_eq!(next["results"].as_array().unwrap().len(), 5);
    assert_eq!(next["truncated"], false);
    assert_ne!(first["results"][2], next["results"][0]);
    let names = execute(&context, json!({"query":"needle"}), false).unwrap();
    assert_eq!(names["results"].as_array().unwrap().len(), 7);
    let all = execute(&context, json!({"query":"needle42"}), true).unwrap();
    assert_eq!(all["results"].as_array().unwrap().len(), 7);
    let selected = execute(
        &context,
        json!({"path":root.join("src/needle.js"),"query":"needle42","include":["*.js"]}),
        true,
    )
    .unwrap();
    assert_eq!(selected["results"].as_array().unwrap().len(), 1);
    context.cancellation.store(true, Ordering::Relaxed);
    assert!(execute(&context, json!({"query":"needle"}), true)
        .unwrap_err()
        .contains("取消"));
    fs::remove_dir_all(root).unwrap();
}

/// files_only must collapse results to one entry per file on BOTH backends,
/// so the model can rely on the shape regardless of whether ripgrep exists.
#[test]
fn files_only_returns_one_entry_per_matching_file_on_every_backend() {
    let root = temporary_root();
    fs::create_dir_all(root.join("src")).unwrap();
    // Two files, each with several matching lines.
    fs::write(root.join("src/a.ts"), "alpha\nalpha\nalpha\n").unwrap();
    fs::write(root.join("src/b.ts"), "alpha\n").unwrap();
    fs::write(root.join("src/c.ts"), "unrelated\n").unwrap();
    let context = LocalToolContext::new(
        vec![root.to_string_lossy().into_owned()],
        vec![],
        false,
        Arc::new(AtomicBool::new(false)),
        None,
    );

    // Line mode: 4 matching lines across 2 files.
    let lines = execute(&context, json!({"query":"alpha"}), true).unwrap();
    assert_eq!(lines["results"].as_array().unwrap().len(), 4);

    // files_only: exactly 2 entries, one per file.
    let files = execute(&context, json!({"query":"alpha","files_only":true}), true).unwrap();
    let results = files["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for entry in results {
        assert_eq!(entry["kind"], "file");
        assert!(entry["path"].as_str().unwrap().ends_with(".ts"));
        // A file entry carries no line numbers: it is not a line match.
        assert!(entry.get("lineStart").is_none());
    }
    assert_eq!(files["truncated"], false);

    // Paging still holds for files_only.
    let first = execute(
        &context,
        json!({"query":"alpha","files_only":true,"max_results":1}),
        true,
    )
    .unwrap();
    assert_eq!(first["results"].as_array().unwrap().len(), 1);
    assert_eq!(first["truncated"], true);
    assert_eq!(first["nextOffset"], 1);

    fs::remove_dir_all(root).unwrap();
}

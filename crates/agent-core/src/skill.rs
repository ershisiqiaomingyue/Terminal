use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_VISITED_DIRECTORIES: usize = 4096;
const MAX_SKILL_FILES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRuntime {
    Instruction,
    Rust,
    Python,
    External,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackage {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub runtime: SkillRuntime,
    pub entrypoint: Option<String>,
    pub dependencies: Vec<String>,
    pub executed_during_scan: bool,
}

#[derive(Debug, Error)]
pub enum SkillScanError {
    #[error("skill repository does not exist: {0}")]
    MissingRepository(String),
    #[error("skill repository is not a directory: {0}")]
    NotDirectory(String),
    #[error("cannot read skill repository {path}: {message}")]
    ReadRepository { path: String, message: String },
    #[error("cannot read skill file {path}: {message}")]
    ReadSkill { path: String, message: String },
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    runtime: Option<String>,
    entrypoint: Option<String>,
    dependencies: Option<Vec<String>>,
    metadata: Option<SkillMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct SkillMetadata {
    runtime: Option<String>,
    dependencies: Option<Vec<String>>,
}

pub fn scan_skill_repository(root: &Path) -> Result<Vec<SkillPackage>, SkillScanError> {
    if !root.exists() {
        return Err(SkillScanError::MissingRepository(
            root.display().to_string(),
        ));
    }
    if !root.is_dir() {
        return Err(SkillScanError::NotDirectory(root.display().to_string()));
    }

    let mut skill_files = Vec::new();
    let mut visited_directories = 0;
    collect_skill_files(root, &mut skill_files, &mut visited_directories).map_err(|error| {
        SkillScanError::ReadRepository {
            path: root.display().to_string(),
            message: error.to_string(),
        }
    })?;
    skill_files.sort();

    let mut packages = Vec::with_capacity(skill_files.len());
    for skill_file in skill_files {
        packages.push(scan_skill_file(root, &skill_file)?);
    }
    Ok(packages)
}

fn collect_skill_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    visited_directories: &mut usize,
) -> std::io::Result<()> {
    *visited_directories += 1;
    if *visited_directories > MAX_VISITED_DIRECTORIES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "skill repository exceeds the {} directory scan limit",
                MAX_VISITED_DIRECTORIES
            ),
        ));
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if matches!(
                name.as_str(),
                ".git" | "target" | "node_modules" | ".venv" | "venv"
            ) {
                continue;
            }
            collect_skill_files(&path, files, visited_directories)?;
        } else if name.eq_ignore_ascii_case("SKILL.md") {
            if files.len() >= MAX_SKILL_FILES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!(
                        "skill repository exceeds the {} skill scan limit",
                        MAX_SKILL_FILES
                    ),
                ));
            }
            files.push(path);
        }
    }
    Ok(())
}

fn scan_skill_file(root: &Path, skill_file: &Path) -> Result<SkillPackage, SkillScanError> {
    let content = fs::read_to_string(skill_file).map_err(|error| SkillScanError::ReadSkill {
        path: skill_file.display().to_string(),
        message: error.to_string(),
    })?;
    let skill_dir = skill_file.parent().unwrap_or(root);
    let relative_dir = relative_path(root, skill_dir);
    let frontmatter = parse_frontmatter(&content).unwrap_or_default();
    let id = frontmatter
        .id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            frontmatter
                .name
                .clone()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| {
            skill_dir
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("unnamed-skill")
                .to_string()
        });
    let name = frontmatter
        .name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| id.clone());
    let description = frontmatter
        .description
        .unwrap_or_default()
        .trim()
        .to_string();
    let discovered_python =
        has_python_files(skill_dir).map_err(|error| SkillScanError::ReadSkill {
            path: skill_dir.display().to_string(),
            message: error.to_string(),
        })? || skill_dir.join("requirements.txt").is_file()
            || skill_dir.join("pyproject.toml").is_file()
            || skill_dir.join("setup.py").is_file();
    let runtime_name = frontmatter.runtime.as_deref().or_else(|| {
        frontmatter
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.runtime.as_deref())
    });
    let runtime = runtime_name.map(parse_runtime).unwrap_or_else(|| {
        if discovered_python {
            SkillRuntime::Python
        } else {
            SkillRuntime::Instruction
        }
    });
    let entrypoint = frontmatter
        .entrypoint
        .filter(|value| !value.trim().is_empty())
        .or_else(|| discovered_python.then(|| find_python_entrypoint(skill_dir, skill_dir)));
    let mut dependencies = frontmatter.dependencies.unwrap_or_else(|| {
        frontmatter
            .metadata
            .and_then(|metadata| metadata.dependencies)
            .unwrap_or_default()
    });
    if runtime == SkillRuntime::Python && !dependencies.iter().any(|value| value == "python") {
        dependencies.insert(0, "python".to_string());
    }
    if runtime == SkillRuntime::Python {
        dependencies.extend(read_python_requirements(skill_dir));
    }
    dependencies = deduplicate(dependencies);

    Ok(SkillPackage {
        id,
        name,
        description,
        path: relative_dir,
        runtime,
        entrypoint,
        dependencies,
        executed_during_scan: false,
    })
}

fn parse_frontmatter(content: &str) -> Option<SkillFrontmatter> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut yaml = String::new();
    for line in lines {
        if line.trim() == "---" {
            return serde_yaml::from_str(&yaml).ok();
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    None
}

fn parse_runtime(value: &str) -> SkillRuntime {
    match value.trim().to_ascii_lowercase().as_str() {
        "rust" => SkillRuntime::Rust,
        "python" => SkillRuntime::Python,
        "external" | "cli" => SkillRuntime::External,
        "mcp" => SkillRuntime::Mcp,
        _ => SkillRuntime::Instruction,
    }
}

fn has_python_files(directory: &Path) -> std::io::Result<bool> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if matches!(name, ".git" | "target" | "node_modules" | ".venv" | "venv") {
                continue;
            }
            if has_python_files(&path)? {
                return Ok(true);
            }
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("py"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn find_python_entrypoint(directory: &Path, root: &Path) -> String {
    let mut candidates = Vec::new();
    collect_python_files(directory, &mut candidates);
    candidates.sort_by_key(|path| (!path.to_string_lossy().contains("scripts"), path.clone()));
    candidates
        .first()
        .map(|path| relative_path(root, path))
        .unwrap_or_default()
}

fn collect_python_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if !matches!(name, ".git" | "target" | "node_modules" | ".venv" | "venv") {
                collect_python_files(&path, files);
            }
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("py"))
        {
            files.push(path);
        }
    }
}

fn read_python_requirements(directory: &Path) -> Vec<String> {
    let path = directory.join("requirements.txt");
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn deduplicate(values: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if !value.is_empty() && !output.iter().any(|existing| existing == &value) {
            output.push(value);
        }
    }
    output
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

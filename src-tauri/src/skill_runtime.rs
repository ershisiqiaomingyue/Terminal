use agent_core::{scan_skill_repository, SkillPackage, SkillRuntime};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use url::Url;
use zip::ZipArchive;

pub(crate) mod management;
use management::{apply_skill_settings, skill_key};
static SKILL_SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const SKILL_CONFIG_FILE: &str = "agent-skills.json";
const SELINUX_SOURCE_CONFIG_FILE: &str = "android-selinux-sources.json";
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 4096;
const MAX_SKILL_INSTRUCTION_BYTES: usize = 128 * 1024;
const DEFAULT_SELINUX_INCLUDE_PATHS: &[&str] = &[
    "system/sepolicy",
    "device",
    "vendor",
    "packages",
    "frameworks",
];
const DEFAULT_SELINUX_EXCLUDE_PATHS: &[&str] = &[
    ".git",
    ".repo",
    "out",
    "build",
    "prebuilts",
    "external",
    "node_modules",
    ".gradle",
    "target",
    "dist",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredSkillSettings {
    repositories: Vec<String>,
    #[serde(default)]
    disabled_skills: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SelinuxSourceConfig {
    pub root: String,
    #[serde(rename = "includePaths", default)]
    pub include_paths: Vec<String>,
    #[serde(rename = "excludePaths", default)]
    pub exclude_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelinuxSourceView {
    pub root: String,
    #[serde(rename = "includePaths")]
    pub include_paths: Vec<String>,
    #[serde(rename = "excludePaths")]
    pub exclude_paths: Vec<String>,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct SelinuxSourceScope {
    pub roots: Vec<String>,
    pub excluded_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredSelinuxSettings {
    sources: Vec<SelinuxSourceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonRuntimeView {
    pub available: bool,
    pub command: Option<String>,
    pub version: Option<String>,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository: String,
    pub path: String,
    pub runtime: SkillRuntime,
    pub entrypoint: Option<String>,
    pub dependencies: Vec<String>,
    pub status: String,
    #[serde(rename = "statusMessage")]
    pub status_message: String,
    pub builtin: bool,
    #[serde(default = "skill_enabled_default")]
    pub enabled: bool,
}

fn skill_enabled_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRepositoryView {
    pub path: String,
    pub status: String,
    pub message: String,
    #[serde(rename = "skillCount")]
    pub skill_count: usize,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCatalogView {
    pub repositories: Vec<SkillRepositoryView>,
    pub skills: Vec<SkillView>,
    pub python: PythonRuntimeView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubRepository {
    owner: String,
    name: String,
    branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFetchRequest {
    pub url: String,
    pub branch: Option<String>,
    pub destination: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFetchView {
    pub owner: String,
    pub name: String,
    pub branch: String,
    pub url: String,
    pub path: String,
    #[serde(rename = "skillCount")]
    pub skill_count: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRemoteCandidate {
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRemotePreview {
    pub owner: String,
    pub name: String,
    pub branch: String,
    pub url: String,
    pub destination: String,
    pub skills: Vec<SkillRemoteCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillImportSelection {
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    pub resolution: String,
    #[serde(rename = "renamedId")]
    pub renamed_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillImportResult {
    pub preview: SkillRemotePreview,
    #[serde(rename = "importedIds")]
    pub imported_ids: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
struct SkillRepositorySource {
    path: PathBuf,
    display_path: String,
    builtin: bool,
}

fn builtin_skill_paths(app: &AppHandle) -> Vec<PathBuf> {
    let candidates = [
        // During development, prefer the checked-out resources so newly added
        // built-in skills are not shadowed by a stale copied resource folder.
        cfg!(debug_assertions).then(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("android-skills")
        }),
        app.path()
            .resource_dir()
            .ok()
            .map(|path| path.join("android-skills")),
        app.path()
            .resource_dir()
            .ok()
            .map(|path| path.join("resources").join("android-skills")),
    ];
    candidates
        .into_iter()
        .flatten()
        .filter(|path| path.is_dir())
        .collect()
}

fn builtin_skill_path(app: &AppHandle) -> Option<PathBuf> {
    builtin_skill_paths(app).into_iter().next()
}

fn skill_sources(app: &AppHandle, repositories: Vec<String>) -> Vec<SkillRepositorySource> {
    let mut sources = Vec::new();
    if let Ok(root) = crate::user_storage::root() {
        if let Some(source) = guidance_skill_source(&root) {
            sources.push(source);
        }
        let path = root.join("skills/development");
        if path.is_dir() {
            sources.push(SkillRepositorySource { display_path:path.to_string_lossy().into_owned(),path,builtin:false });
        }
    }
    if let Some(path) = builtin_skill_path(app) {
        sources.push(SkillRepositorySource {
            path,
            display_path: "内置 Android Skills".to_string(),
            builtin: true,
        });
    }
    let mut seen = sources
        .iter()
        .filter_map(|source| source.path.canonicalize().ok())
        .collect::<HashSet<_>>();
    for repository in repositories {
        let path = PathBuf::from(&repository);
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(key) {
            sources.push(SkillRepositorySource {
                path,
                display_path: repository,
                builtin: false,
            });
        }
    }
    sources
}

fn guidance_skill_source(user_root: &Path) -> Option<SkillRepositorySource> {
    let path = user_root.join("skills/guidance");
    path.is_dir().then(|| SkillRepositorySource {
        display_path: path.to_string_lossy().into_owned(),
        path,
        builtin: false,
    })
}

fn parse_github_repository(raw: &str) -> Result<GitHubRepository, String> {
    let parsed = Url::parse(raw.trim()).map_err(|_| "GitHub 仓库 URL 无法解析".to_string())?;
    if parsed.scheme() != "https" {
        return Err("远程 Skills 仓库只允许使用 https://".to_string());
    }
    let host = parsed.host_str().unwrap_or_default();
    if host != "github.com" && host != "www.github.com" {
        return Err("当前只支持 github.com 仓库".to_string());
    }
    let segments = parsed
        .path_segments()
        .ok_or_else(|| "GitHub 仓库 URL 缺少路径".to_string())?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 2 {
        return Err("GitHub 仓库 URL 必须包含 owner/repository".to_string());
    }
    let owner = segments[0].to_string();
    let name = segments[1]
        .strip_suffix(".git")
        .unwrap_or(segments[1])
        .to_string();
    if !is_safe_repo_segment(&owner) || !is_safe_repo_segment(&name) {
        return Err("GitHub owner 或 repository 名称无效".to_string());
    }
    let branch = if segments.get(2) == Some(&"tree") && segments.len() >= 4 {
        let branch = segments[3..].join("/");
        validate_branch(&branch)?;
        branch
    } else {
        "main".to_string()
    };
    Ok(GitHubRepository {
        owner,
        name,
        branch,
    })
}

fn is_safe_repo_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn validate_branch(branch: &str) -> Result<(), String> {
    if branch.trim().is_empty()
        || branch.contains("..")
        || !branch.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
        })
    {
        return Err("GitHub branch 名称无效".to_string());
    }
    Ok(())
}

fn validate_archive_member(member: &str) -> Result<(), String> {
    let normalized = member.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(format!("压缩包成员包含绝对路径: {member}"));
    }
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::Prefix(_)
                | std::path::Component::RootDir
                | std::path::Component::ParentDir
        ) {
            return Err(format!("压缩包成员路径不安全: {member}"));
        }
    }
    Ok(())
}

fn archive_root_name(member: &str) -> Option<&str> {
    member.split('/').find(|segment| !segment.is_empty())
}

fn archive_relative_member(member: &str, root: Option<&str>) -> Result<String, String> {
    validate_archive_member(member)?;
    let normalized = member.replace('\\', "/").trim_matches('/').to_string();
    let relative = root
        .and_then(|root| normalized.strip_prefix(&format!("{root}/")))
        .unwrap_or(&normalized);
    if relative.is_empty() {
        return Err("压缩包成员路径为空".to_string());
    }
    validate_archive_member(relative)?;
    Ok(relative.to_string())
}

fn sanitize_skill_id(raw: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut dash = false;
    for character in raw.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character);
            dash = false;
        } else if !dash {
            result.push('-');
            dash = true;
        }
    }
    let result = result.trim_matches('-').to_string();
    if result.is_empty() {
        return Err(format!("无法生成 skill id: {raw}"));
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
struct RemoteSkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn parse_remote_skill_markdown(content: &str, fallback_name: &str) -> (String, Option<String>) {
    let Some(start) = content.lines().next().filter(|line| line.trim() == "---") else {
        return (fallback_name.to_string(), None);
    };
    let _ = start;
    let mut yaml = String::new();
    for line in content.lines().skip(1) {
        if line.trim() == "---" {
            if let Ok(frontmatter) = serde_yaml::from_str::<RemoteSkillFrontmatter>(&yaml) {
                return (
                    frontmatter
                        .name
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| fallback_name.to_string()),
                    frontmatter
                        .description
                        .filter(|value| !value.trim().is_empty()),
                );
            }
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    (fallback_name.to_string(), None)
}

async fn download_github_archive(
    repository: &GitHubRepository,
) -> Result<(Vec<u8>, String), String> {
    let client = crate::android_tools::configured_http_client_builder()?
        .user_agent("android-tool-skills")
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|error| format!("无法创建 GitHub 请求: {error}"))?;
    let mut branches = vec![repository.branch.clone()];
    if repository.branch == "main" {
        branches.push("master".to_string());
    }
    let mut last_error = String::new();
    for branch in branches {
        let endpoint = format!(
            "https://codeload.github.com/{}/{}/zip/refs/heads/{}",
            repository.owner, repository.name, branch
        );
        let response = client
            .get(&endpoint)
            .send()
            .await
            .map_err(|error| format!("GitHub 网络请求失败: {error}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            last_error = format!("仓库或 branch 不存在: {branch}");
            continue;
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(
                if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
                    "GitHub 拒绝了请求，可能触发限流或仓库需要权限。".to_string()
                } else {
                    format!("GitHub 返回 HTTP {}: {}", status.as_u16(), body.trim())
                },
            );
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_ARCHIVE_BYTES)
        {
            return Err(format!(
                "远程仓库压缩包超过 {} MB 限制",
                MAX_ARCHIVE_BYTES / 1024 / 1024
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("读取 GitHub 压缩包失败: {error}"))?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(format!(
                "远程仓库压缩包超过 {} MB 限制",
                MAX_ARCHIVE_BYTES / 1024 / 1024
            ));
        }
        return Ok((bytes.to_vec(), branch));
    }
    Err(last_error)
}

fn preview_archive(
    repository: &GitHubRepository,
    bytes: &[u8],
    destination: &Path,
) -> Result<SkillRemotePreview, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("GitHub 压缩包无法打开: {error}"))?;
    if archive.len() > MAX_ARCHIVE_FILES {
        return Err(format!("远程仓库文件数超过 {} 限制", MAX_ARCHIVE_FILES));
    }
    let root = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_string())
        })
        .find_map(|name| archive_root_name(&name).map(str::to_string));
    let mut skills = Vec::new();
    let mut seen = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 GitHub 文件失败: {error}"))?;
        if entry.is_dir()
            || !entry.name().replace('\\', "/").ends_with("/SKILL.md") && entry.name() != "SKILL.md"
        {
            continue;
        }
        let relative = archive_relative_member(entry.name(), root.as_deref())?;
        let source_path = Path::new(&relative)
            .parent()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if !seen.insert(source_path.clone()) {
            continue;
        }
        let fallback = Path::new(&source_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("skill");
        let mut content = Vec::new();
        entry
            .take(1024 * 1024)
            .read_to_end(&mut content)
            .map_err(|error| format!("读取 SKILL.md 失败: {error}"))?;
        let (name, description) =
            parse_remote_skill_markdown(&String::from_utf8_lossy(&content), fallback);
        let id = sanitize_skill_id(&name)?;
        skills.push(SkillRemoteCandidate {
            source_path,
            id: id.clone(),
            name,
            description,
            conflict: destination.join(id).exists(),
        });
    }
    skills.sort_by(|left, right| left.source_path.cmp(&right.source_path));
    if skills.is_empty() {
        return Err("仓库中没有找到可导入的 SKILL.md".to_string());
    }
    Ok(SkillRemotePreview {
        owner: repository.owner.clone(),
        name: repository.name.clone(),
        branch: repository.branch.clone(),
        url: format!(
            "https://github.com/{}/{}",
            repository.owner, repository.name
        ),
        destination: destination.to_string_lossy().into_owned(),
        skills,
    })
}

fn extract_selected_skill(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    root: Option<&str>,
    source_path: &str,
    destination: &Path,
) -> Result<u64, String> {
    fs::create_dir_all(destination).map_err(|error| format!("创建 skill 临时目录失败: {error}"))?;
    let mut extracted_bytes = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 GitHub 文件失败: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let relative = archive_relative_member(entry.name(), root)?;
        let selected = if source_path.is_empty() {
            !relative.contains('/')
        } else {
            relative == source_path || relative.starts_with(&format!("{source_path}/"))
        };
        if !selected {
            continue;
        }
        let target_relative = if source_path.is_empty() {
            relative.clone()
        } else {
            relative
                .strip_prefix(&format!("{source_path}/"))
                .unwrap_or(&relative)
                .to_string()
        };
        let target = destination.join(&target_relative);
        if !target.starts_with(destination) {
            return Err("skill 文件路径越界".to_string());
        }
        extracted_bytes = extracted_bytes.saturating_add(entry.size());
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err(format!(
                "skill 解压内容超过 {} MB 限制",
                MAX_EXTRACTED_BYTES / 1024 / 1024
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("创建 skill 目录失败: {error}"))?;
        }
        let mut output =
            fs::File::create(&target).map_err(|error| format!("写入 skill 文件失败: {error}"))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("解压 skill 文件失败: {error}"))?;
        output
            .flush()
            .map_err(|error| format!("刷新 skill 文件失败: {error}"))?;
    }
    if !destination.join("SKILL.md").is_file() {
        return Err("导入结果缺少 SKILL.md".to_string());
    }
    Ok(extracted_bytes)
}

#[tauri::command]
pub async fn preview_skill_repository(
    app: AppHandle,
    request: SkillFetchRequest,
) -> Result<SkillRemotePreview, String> {
    let repository = parse_github_repository(&request.url)?;
    let repository = if let Some(branch) = request
        .branch
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        validate_branch(branch)?;
        GitHubRepository {
            branch: branch.to_string(),
            ..repository
        }
    } else {
        repository
    };
    let destination = management::import_destination(&app, &request.destination)?;
    let (bytes, branch) = download_github_archive(&repository).await?;
    let repository = GitHubRepository {
        branch,
        ..repository
    };
    tauri::async_runtime::spawn_blocking(move || preview_archive(&repository, &bytes, &destination))
        .await
        .map_err(|error| format!("GitHub Skills 预览任务失败: {error}"))?
}

#[tauri::command]
pub async fn import_skill_repository(
    app: AppHandle,
    request: SkillFetchRequest,
    selections: Vec<SkillImportSelection>,
) -> Result<SkillImportResult, String> {
    let destination = management::import_destination(&app, &request.destination)?;
    let repository = parse_github_repository(&request.url)?;
    let repository = if let Some(branch) = request
        .branch
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        validate_branch(branch)?;
        GitHubRepository {
            branch: branch.to_string(),
            ..repository
        }
    } else {
        repository
    };
    let (bytes, branch) = download_github_archive(&repository).await?;
    let repository = GitHubRepository {
        branch,
        ..repository
    };
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = SKILL_SETTINGS_LOCK
            .lock()
            .map_err(|_| "Skills 配置锁不可用")?;
        let result = import_skill_archive(repository, bytes, selections, destination.clone())?;
        if !result.imported_ids.is_empty() {
            let mut settings = load_settings_unlocked(&app)?;
            let path = destination.to_string_lossy().into_owned();
            if !settings.repositories.contains(&path) {
                settings.repositories.push(path);
            }
            save_settings(&app, &settings)?;
        }
        Ok(result)
    })
    .await
    .map_err(|error| format!("GitHub Skills 导入任务失败: {error}"))?
}

fn import_skill_archive(
    repository: GitHubRepository,
    bytes: Vec<u8>,
    selections: Vec<SkillImportSelection>,
    destination: PathBuf,
) -> Result<SkillImportResult, String> {
    let preview = preview_archive(&repository, &bytes, &destination)?;
    if selections.is_empty() {
        return Err("请至少选择一个 skill 导入".to_string());
    }
    let root = {
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice()))
            .map_err(|error| format!("GitHub 压缩包无法打开: {error}"))?;
        (0..archive.len())
            .filter_map(|index| {
                archive
                    .by_index(index)
                    .ok()
                    .map(|entry| entry.name().to_string())
            })
            .find_map(|name| archive_root_name(&name).map(str::to_string))
    };
    let mut imported_ids = Vec::new();
    let mut skipped = Vec::new();
    for selection in selections {
        let candidate = preview
            .skills
            .iter()
            .find(|skill| skill.source_path == selection.source_path)
            .ok_or_else(|| format!("未找到 skill: {}", selection.source_path))?;
        let resolution = selection.resolution.trim().to_ascii_lowercase();
        if resolution == "skip" {
            skipped.push(candidate.id.clone());
            continue;
        }
        let id = if resolution == "rename" {
            sanitize_skill_id(
                selection
                    .renamed_id
                    .as_deref()
                    .ok_or_else(|| "重命名导入需要提供 renamedId".to_string())?,
            )?
        } else {
            candidate.id.clone()
        };
        let target = destination.join(&id);
        management::validate_import_child(&destination, &target)?;
        if target.exists() {
            if resolution != "overwrite" {
                skipped.push(candidate.id.clone());
                continue;
            }
        }
        let staging = destination.join(format!(".{id}.importing"));
        management::validate_import_child(&destination, &staging)?;
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(|error| format!("清理上次导入失败: {error}"))?;
        }
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice()))
            .map_err(|error| format!("GitHub 压缩包无法打开: {error}"))?;
        if let Err(error) = extract_selected_skill(
            &mut archive,
            root.as_deref(),
            &candidate.source_path,
            &staging,
        ) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        // Fully validate/extract before replacing the previous working definition.
        if target.exists() {
            fs::remove_dir_all(&target).map_err(|error| format!("覆盖已有 skill 失败: {error}"))?;
        }
        fs::rename(&staging, &target).map_err(|error| format!("完成 skill 导入失败: {error}"))?;
        imported_ids.push(id);
    }
    Ok(SkillImportResult {
        preview,
        imported_ids,
        skipped,
    })
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(SKILL_CONFIG_FILE))
        .map_err(|error| format!("无法定位 Skills 配置目录: {error}"))
}

fn selinux_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(SELINUX_SOURCE_CONFIG_FILE))
        .map_err(|error| format!("无法定位 SELinux 配置目录: {error}"))
}

fn normalize_selinux_relative_path(raw: &str) -> Result<Option<String>, String> {
    let value = raw.trim().replace('\\', "/");
    if value.is_empty() {
        return Ok(None);
    }
    if value.contains(':') || value.starts_with('/') || value.starts_with("//") {
        return Err(format!("SELinux 路径必须是源码根目录下的相对路径: {raw}"));
    }
    let path = Path::new(&value);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                let part = part.to_string_lossy();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::Prefix(_)
            | std::path::Component::RootDir => {
                return Err(format!("SELinux 路径不能越过源码根目录: {raw}"));
            }
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join("/")))
}

fn normalize_selinux_relative_paths(
    values: Vec<String>,
    defaults: &[&str],
) -> Result<Vec<String>, String> {
    let values = if values.is_empty() {
        defaults.iter().map(|value| (*value).to_string()).collect()
    } else {
        values
    };
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for value in values {
        if let Some(value) = normalize_selinux_relative_path(&value)? {
            if seen.insert(value.clone()) {
                normalized.push(value);
            }
        }
    }
    Ok(normalized)
}

fn normalize_selinux_source(source: SelinuxSourceConfig) -> Result<SelinuxSourceConfig, String> {
    let root = PathBuf::from(source.root.trim());
    if !root.is_absolute() {
        return Err(format!("SELinux 源码根目录必须是绝对路径: {}", source.root));
    }
    let root = if root.exists() {
        root.canonicalize().unwrap_or(root)
    } else {
        root
    };
    Ok(SelinuxSourceConfig {
        root: root.to_string_lossy().to_string(),
        include_paths: normalize_selinux_relative_paths(
            source.include_paths,
            DEFAULT_SELINUX_INCLUDE_PATHS,
        )?,
        exclude_paths: normalize_selinux_relative_paths(
            source.exclude_paths,
            DEFAULT_SELINUX_EXCLUDE_PATHS,
        )?,
    })
}

fn load_selinux_sources(app: &AppHandle) -> Result<Vec<SelinuxSourceConfig>, String> {
    let path = selinux_config_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(&path).map_err(|error| format!("无法读取 SELinux 源码配置: {error}"))?;
    let stored = serde_json::from_str::<StoredSelinuxSettings>(&content)
        .map_err(|error| format!("SELinux 源码配置格式无效: {error}"))?;
    stored
        .sources
        .into_iter()
        .map(normalize_selinux_source)
        .collect()
}

fn save_selinux_sources_config(
    app: &AppHandle,
    sources: &[SelinuxSourceConfig],
) -> Result<(), String> {
    let path = selinux_config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "SELinux 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 SELinux 配置目录: {error}"))?;
    let content = serde_json::to_string_pretty(&StoredSelinuxSettings {
        sources: sources.to_vec(),
    })
    .map_err(|error| format!("无法序列化 SELinux 源码配置: {error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存 SELinux 源码配置: {error}"))
}

fn selinux_source_view(source: SelinuxSourceConfig) -> SelinuxSourceView {
    let path = Path::new(&source.root);
    let (status, message) = match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => (
            "ready".to_string(),
            format!(
                "已配置 {} 个检索目录，执行时按需读取",
                source.include_paths.len()
            ),
        ),
        Ok(_) => ("invalid".to_string(), "源码根目录不是文件夹".to_string()),
        Err(_) => (
            "missing".to_string(),
            "源码根目录不存在或暂时不可访问".to_string(),
        ),
    };
    SelinuxSourceView {
        root: source.root,
        include_paths: source.include_paths,
        exclude_paths: source.exclude_paths,
        status,
        message,
    }
}

pub fn load_selinux_source_scope(app: &AppHandle) -> Result<SelinuxSourceScope, String> {
    let mut scope = SelinuxSourceScope::default();
    let mut roots = HashSet::new();
    let mut excluded_paths = HashSet::new();
    for source in load_selinux_sources(app)? {
        let source_root = PathBuf::from(&source.root);
        if !source_root.is_dir() {
            continue;
        }
        let source_root = source_root.canonicalize().unwrap_or(source_root);
        let source_excludes = source
            .exclude_paths
            .iter()
            .map(|exclude| {
                let path = source_root.join(exclude);
                path.canonicalize().unwrap_or(path)
            })
            .collect::<Vec<_>>();
        for include in &source.include_paths {
            let path = source_root.join(include);
            if path.is_dir()
                && !source_excludes
                    .iter()
                    .any(|exclude| path.starts_with(exclude))
            {
                let path = path.canonicalize().unwrap_or(path);
                let value = path.to_string_lossy().to_string();
                if roots.insert(value.clone()) {
                    scope.roots.push(value);
                }
            }
        }
        for path in source_excludes {
            let value = path.to_string_lossy().to_string();
            if excluded_paths.insert(value.clone()) {
                scope.excluded_paths.push(value);
            }
        }
    }
    scope.roots.sort_by_key(|value| value.to_ascii_lowercase());
    scope
        .excluded_paths
        .sort_by_key(|value| value.to_ascii_lowercase());
    Ok(scope)
}

#[tauri::command]
pub fn list_selinux_sources(app: AppHandle) -> Result<Vec<SelinuxSourceView>, String> {
    load_selinux_sources(&app).map(|sources| sources.into_iter().map(selinux_source_view).collect())
}

#[tauri::command]
pub fn save_selinux_sources(
    app: AppHandle,
    sources: Vec<SelinuxSourceConfig>,
) -> Result<Vec<SelinuxSourceView>, String> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for source in sources {
        let source = normalize_selinux_source(source)?;
        if seen.insert(source.root.clone()) {
            normalized.push(source);
        }
    }
    save_selinux_sources_config(&app, &normalized)?;
    Ok(normalized.into_iter().map(selinux_source_view).collect())
}

fn load_repositories(app: &AppHandle) -> Result<Vec<String>, String> {
    Ok(normalize_repositories(load_settings(app)?.repositories))
}

fn load_settings(app: &AppHandle) -> Result<StoredSkillSettings, String> {
    let _guard = SKILL_SETTINGS_LOCK
        .lock()
        .map_err(|_| "Skills 配置锁不可用")?;
    load_settings_unlocked(app)
}

fn load_settings_unlocked(app: &AppHandle) -> Result<StoredSkillSettings, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(StoredSkillSettings::default());
    }
    let content =
        fs::read_to_string(&path).map_err(|error| format!("无法读取 Skills 配置: {error}"))?;
    let stored = serde_json::from_str::<StoredSkillSettings>(&content)
        .map_err(|error| format!("Skills 配置格式无效: {error}"))?;
    Ok(stored)
}

fn save_repositories(app: &AppHandle, repositories: &[String]) -> Result<(), String> {
    let _guard = SKILL_SETTINGS_LOCK
        .lock()
        .map_err(|_| "Skills 配置锁不可用")?;
    let mut settings = load_settings_unlocked(app)?;
    settings.repositories = repositories.to_vec();
    save_settings(app, &settings)
}

fn save_settings(app: &AppHandle, settings: &StoredSkillSettings) -> Result<(), String> {
    let path = config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "Skills 配置目录无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建 Skills 配置目录: {error}"))?;
    let content = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("无法序列化 Skills 配置: {error}"))?;
    fs::write(path, content).map_err(|error| format!("无法保存 Skills 配置: {error}"))
}

fn normalize_repositories(repositories: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    repositories
        .into_iter()
        .filter_map(|value| {
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return None;
            }
            let normalized = if path.exists() {
                path.canonicalize().unwrap_or(path)
            } else {
                path
            };
            let value = normalized.to_string_lossy().to_string();
            seen.insert(value.clone()).then_some(value)
        })
        .collect()
}

fn detect_python_runtime() -> PythonRuntimeView {
    if let Ok((command, text)) = crate::android_tools::inspect_python_runtime() {
        if let Some(version) = parse_python_version(&text) {
            return PythonRuntimeView {
                available: true,
                command: Some(command.to_string_lossy().into_owned()),
                version: Some(version.clone()),
                status: "available".to_string(),
                message: format!("已检测到 Python {version}"),
            };
        }
    }

    PythonRuntimeView {
        available: false,
        command: None,
        version: None,
        status: "missing".to_string(),
        message: "未检测到 Python，请在设置的环境与工具中下载或选择本地解释器。".to_string(),
    }
}

fn parse_python_version(value: &str) -> Option<String> {
    let marker = "Python ";
    let start = value.find(marker)? + marker.len();
    let version = value[start..]
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| !character.is_ascii_digit() && character != '.');
    let components = version.split('.').collect::<Vec<_>>();
    (components.len() >= 2
        && components[0].parse::<u32>().is_ok()
        && components[1].parse::<u32>().is_ok())
    .then_some(version.to_string())
}

fn skill_view(
    repository: &str,
    builtin: bool,
    package: SkillPackage,
    python: &PythonRuntimeView,
) -> SkillView {
    let (status, status_message) = match package.runtime {
        SkillRuntime::Instruction => ("ready", "可直接作为 Agent 知识加载".to_string()),
        SkillRuntime::Python if python.available => (
            "detected",
            format!("{}，Python 执行适配器待启用", python.message),
        ),
        SkillRuntime::Python => ("needs_python", python.message.clone()),
        SkillRuntime::Rust => ("not_ready", "Rust skill 执行器尚未接入".to_string()),
        SkillRuntime::External => ("needs_external", "需要检测并配置外部 CLI".to_string()),
        SkillRuntime::Mcp => ("needs_mcp", "需要连接对应 MCP 服务".to_string()),
    };

    SkillView {
        id: package.id,
        name: package.name,
        description: package.description,
        repository: repository.to_string(),
        path: package.path,
        runtime: package.runtime,
        entrypoint: package.entrypoint,
        dependencies: package.dependencies,
        status: status.to_string(),
        status_message,
        builtin,
        enabled: true,
    }
}

fn scan_catalog(repositories: Vec<SkillRepositorySource>) -> SkillCatalogView {
    let python = detect_python_runtime();
    scan_catalog_with_python(repositories, python)
}

fn scan_catalog_with_python(
    repositories: Vec<SkillRepositorySource>,
    python: PythonRuntimeView,
) -> SkillCatalogView {
    let mut repository_views = Vec::new();
    let mut skills = Vec::new();

    for repository in repositories {
        let path = repository.path;
        let display_path = repository.display_path;
        let builtin = repository.builtin;
        if !path.exists() {
            repository_views.push(SkillRepositoryView {
                path: display_path,
                status: "missing".to_string(),
                message: "目录不存在".to_string(),
                skill_count: 0,
                builtin,
            });
            continue;
        }
        if !path.is_dir() {
            repository_views.push(SkillRepositoryView {
                path: display_path,
                status: "invalid".to_string(),
                message: "路径不是目录".to_string(),
                skill_count: 0,
                builtin,
            });
            continue;
        }
        match scan_skill_repository(&path) {
            Ok(packages) => {
                let start = skills.len();
                skills.extend(
                    packages
                        .into_iter()
                        .map(|package| skill_view(&display_path, builtin, package, &python)),
                );
                repository_views.push(SkillRepositoryView {
                    path: display_path,
                    status: "ready".to_string(),
                    message: if builtin {
                        "应用内置，未执行 skill 脚本"
                    } else {
                        "已完成元数据扫描，未执行 skill 脚本"
                    }
                    .to_string(),
                    skill_count: skills.len() - start,
                    builtin,
                });
            }
            Err(error) => repository_views.push(SkillRepositoryView {
                path: display_path,
                status: "invalid".to_string(),
                message: error.to_string(),
                skill_count: 0,
                builtin,
            }),
        }
    }

    SkillCatalogView {
        repositories: repository_views,
        skills,
        python,
    }
}

#[tauri::command]
pub async fn list_skills(app: AppHandle) -> Result<SkillCatalogView, String> {
    let settings = load_settings(&app)?;
    let repositories = skill_sources(&app, normalize_repositories(settings.repositories.clone()));
    tauri::async_runtime::spawn_blocking(move || {
        let mut catalog = scan_catalog(repositories);
        apply_skill_settings(&mut catalog.skills, &settings);
        catalog
    })
    .await
    .map_err(|error| format!("Skills 扫描任务失败: {error}"))
}

pub fn load_skill_catalog(app: &AppHandle) -> Result<Vec<SkillView>, String> {
    let settings = load_settings(app)?;
    let repositories = skill_sources(app, normalize_repositories(settings.repositories.clone()));
    // Conversation discovery reads metadata only, without launching interpreters.
    let mut skills = scan_catalog_with_python(
        repositories,
        PythonRuntimeView {
            available: false,
            command: None,
            version: None,
            status: "not_checked".into(),
            message: "Python 执行适配器尚未接入".into(),
        },
    )
    .skills;
    apply_skill_settings(&mut skills, &settings);
    skills.retain(|skill| skill.enabled);
    Ok(skills)
}

pub fn load_skill_instructions(
    app: &AppHandle,
    ids: &[String],
) -> Result<Vec<(String, String)>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let settings = load_settings(app)?;
    load_instructions_from_sources(
        skill_sources(app, normalize_repositories(settings.repositories)),
        ids,
        &settings.disabled_skills,
    )
}

fn load_instructions_from_sources(
    sources: Vec<SkillRepositorySource>,
    ids: &[String],
    disabled: &HashSet<String>,
) -> Result<Vec<(String, String)>, String> {
    let wanted = ids
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut loaded = HashMap::new();
    for source in sources {
        if !source.path.is_dir() {
            continue;
        }
        let packages = scan_skill_repository(&source.path)
            .map_err(|error| format!("扫描 Skill 仓库失败：{}：{error}", source.display_path))?;
        let root = source
            .path
            .canonicalize()
            .map_err(|error| format!("无法定位 Skill 仓库：{error}"))?;
        for package in packages {
            if disabled.contains(&skill_key(&source.display_path, &package.path)) {
                continue;
            }
            if !wanted.contains(&package.id) || loaded.contains_key(&package.id) {
                continue;
            }
            if package.runtime != SkillRuntime::Instruction {
                return Err(format!("Skill {} 当前只支持 instruction 类型", package.id));
            }
            let skill_path = root.join(&package.path).join("SKILL.md");
            let skill_path = skill_path
                .canonicalize()
                .map_err(|error| format!("无法读取 Skill {}：{error}", package.id))?;
            if !skill_path.starts_with(&root) {
                return Err(format!("Skill {} 路径越界", package.id));
            }
            let bytes = fs::read(&skill_path)
                .map_err(|error| format!("无法读取 Skill {}：{error}", package.id))?;
            if bytes.len() > MAX_SKILL_INSTRUCTION_BYTES {
                return Err(format!(
                    "Skill {} 内容超过 {} KB 限制",
                    package.id,
                    MAX_SKILL_INSTRUCTION_BYTES / 1024
                ));
            }
            loaded.insert(package.id, String::from_utf8_lossy(&bytes).to_string());
        }
    }
    let missing = wanted
        .iter()
        .filter(|id| !loaded.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("未找到已启用的 Skill：{}", missing.join("、")));
    }
    Ok(ids
        .iter()
        .filter_map(|id| loaded.get(id).map(|content| (id.clone(), content.clone())))
        .collect())
}

#[tauri::command]
pub async fn save_skill_repositories(
    app: AppHandle,
    repositories: Vec<String>,
) -> Result<SkillCatalogView, String> {
    let repositories = normalize_repositories(repositories);
    save_repositories(&app, &repositories)?;
    list_skills(app).await
}

#[cfg(test)]
mod tests {
    use super::{
        extract_selected_skill, parse_github_repository, parse_python_version, preview_archive,
        skill_view, validate_archive_member, GitHubRepository, PythonRuntimeView,
    };
    use agent_core::{SkillPackage, SkillRuntime};
    use std::io::{Cursor, Write};
    use std::path::Path;
    use zip::ZipArchive;

    #[test]
    fn editable_guidance_is_metadata_until_selected_and_obeys_edits_disable_and_delete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let guidance = root.join("skills/guidance");
        crate::user_storage::copy_missing_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/agent-instructions/skills"),
            &guidance,
        ).unwrap();
        let sources = || vec![super::guidance_skill_source(root).unwrap()];
        let packages = agent_core::scan_skill_repository(&guidance).unwrap();
        assert_eq!(packages.len(), 2);
        let metadata = serde_json::to_string(&packages).unwrap();
        assert!(metadata.contains("Yocto"));
        assert!(!metadata.contains("capture_android_logcat"));
        assert!(!metadata.contains("android_archive_edit"));
        assert!(super::load_instructions_from_sources(sources(), &[], &Default::default()).unwrap().is_empty());

        let ids = vec!["android-task-guidance".into()];
        let loaded = super::load_instructions_from_sources(sources(), &ids, &Default::default()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].1.contains("capture_android_logcat"));
        assert!(!loaded[0].1.contains("# Yocto 任务指导"));

        let file = guidance.join("android-task-guidance/SKILL.md");
        let edited = "---\nname: android-task-guidance\ndescription: 用户自定义\nruntime: instruction\n---\n新的专用指导";
        std::fs::write(&file, edited).unwrap();
        assert_eq!(super::load_instructions_from_sources(sources(), &ids, &Default::default()).unwrap()[0].1, edited);
        let disabled = [super::skill_key(&guidance.to_string_lossy(), "android-task-guidance")].into_iter().collect();
        assert!(super::load_instructions_from_sources(sources(), &ids, &disabled).is_err());
        std::fs::remove_file(&file).unwrap();
        assert!(super::load_instructions_from_sources(sources(), &ids, &Default::default()).is_err());
        assert_eq!(agent_core::scan_skill_repository(&guidance).unwrap().len(), 1);
    }

    #[test]
    fn loads_only_selected_instructions_and_never_executes_discovered_scripts() {
        let root = std::env::temp_dir().join(format!(
            "android-skill-load-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::create_dir_all(root.join("script")).unwrap();
        std::fs::write(
            root.join("build/SKILL.md"),
            "---\nname: build\ndescription: build app\nruntime: instruction\n---\n检查环境再编译",
        )
        .unwrap();
        std::fs::write(
            root.join("script/SKILL.md"),
            "---\nname: script\nruntime: python\n---\nPython script",
        )
        .unwrap();
        std::fs::write(
            root.join("script/run.py"),
            "raise RuntimeError('must not run during discovery')",
        )
        .unwrap();
        let source = || {
            vec![super::SkillRepositorySource {
                path: root.clone(),
                display_path: root.display().to_string(),
                builtin: false,
            }]
        };
        let loaded =
            super::load_instructions_from_sources(source(), &["build".into()], &Default::default())
                .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, "build");
        assert!(loaded[0].1.contains("检查环境再编译"));
        let disabled = [super::skill_key(&root.display().to_string(), "build")]
            .into_iter()
            .collect();
        assert!(
            super::load_instructions_from_sources(source(), &["build".into()], &disabled)
                .unwrap_err()
                .contains("未找到已启用")
        );
        assert!(super::load_instructions_from_sources(
            source(),
            &["script".into()],
            &Default::default()
        )
        .unwrap_err()
        .contains("instruction"));
        assert!(super::load_instructions_from_sources(
            source(),
            &["missing".into()],
            &Default::default()
        )
        .unwrap_err()
        .contains("未找到"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_python_version_from_stdout_or_stderr() {
        assert_eq!(
            parse_python_version("Python 3.12.4\n"),
            Some("3.12.4".to_string())
        );
        assert_eq!(
            parse_python_version("Python 3.11.9\n"),
            Some("3.11.9".to_string())
        );
        assert_eq!(parse_python_version("python unavailable"), None);
    }

    #[test]
    fn python_skill_without_runtime_is_blocked_before_execution() {
        let view = skill_view(
            "C:/skills",
            false,
            SkillPackage {
                id: "python-skill".to_string(),
                name: "Python skill".to_string(),
                description: String::new(),
                path: "python-skill".to_string(),
                runtime: SkillRuntime::Python,
                entrypoint: Some("run.py".to_string()),
                dependencies: vec!["python".to_string()],
                executed_during_scan: false,
            },
            &PythonRuntimeView {
                available: false,
                command: None,
                version: None,
                status: "missing".to_string(),
                message: "Python missing".to_string(),
            },
        );

        assert_eq!(view.status, "needs_python");
        assert_eq!(view.status_message, "Python missing");
    }

    #[test]
    fn accepts_public_github_repository_urls_and_defaults_branch() {
        let repository = parse_github_repository("https://github.com/android/skills")
            .expect("public GitHub URL should be accepted");

        assert_eq!(repository.owner, "android");
        assert_eq!(repository.name, "skills");
        assert_eq!(repository.branch, "main");
    }

    #[test]
    fn rejects_non_github_repository_urls() {
        assert!(parse_github_repository("http://github.com/android/skills").is_err());
        assert!(parse_github_repository("https://example.com/android/skills").is_err());
        assert!(parse_github_repository("https://github.com/android").is_err());
    }

    #[test]
    fn rejects_zip_members_that_escape_the_destination() {
        assert!(validate_archive_member("repo-main/skill/SKILL.md").is_ok());
        assert!(validate_archive_member("../outside.txt").is_err());
        assert!(validate_archive_member("repo-main/../../outside.txt").is_err());
        assert!(validate_archive_member("C:/outside.txt").is_err());
    }

    #[test]
    fn previews_repo_root_and_nested_skills_without_importing_the_whole_repo() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("skills-main/SKILL.md", options)
            .expect("root skill");
        writer
            .write_all(b"---\nname: Root Skill\ndescription: root docs\n---\n")
            .expect("root metadata");
        writer
            .start_file("skills-main/skills/android/SKILL.md", options)
            .expect("nested skill");
        writer
            .write_all(b"---\nname: Android Skill\ndescription: android docs\n---\n")
            .expect("nested metadata");
        writer
            .start_file("skills-main/skills/android/README.md", options)
            .expect("nested file");
        writer.write_all(b"readme").expect("nested content");
        let archive = writer.finish().expect("archive should finish").into_inner();

        let preview = preview_archive(
            &GitHubRepository {
                owner: "example".to_string(),
                name: "skills".to_string(),
                branch: "main".to_string(),
            },
            &archive,
            Path::new("C:/destination"),
        )
        .expect("fixture should contain importable skills");

        assert_eq!(preview.skills.len(), 2);
        assert_eq!(preview.skills[0].source_path, "");
        assert_eq!(preview.skills[0].name, "Root Skill");
        assert_eq!(preview.skills[1].source_path, "skills/android");
        assert_eq!(
            preview.skills[1].description.as_deref(),
            Some("android docs")
        );
    }

    #[test]
    fn preview_rejects_archive_path_traversal() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "repo-main/../outside/SKILL.md",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("unsafe fixture member");
        writer
            .write_all(b"# unsafe")
            .expect("unsafe fixture content");
        let archive = writer.finish().expect("archive should finish").into_inner();
        let result = preview_archive(
            &GitHubRepository {
                owner: "example".to_string(),
                name: "skills".to_string(),
                branch: "main".to_string(),
            },
            &archive,
            Path::new("C:/destination"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn extracts_only_the_selected_skill_directory() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("repo-main/skills/one/SKILL.md", options)
            .expect("selected skill");
        writer.write_all(b"# one").expect("selected file");
        writer
            .start_file("repo-main/skills/one/README.md", options)
            .expect("selected readme");
        writer
            .write_all(b"readme")
            .expect("selected readme content");
        writer
            .start_file("repo-main/skills/two/SKILL.md", options)
            .expect("other skill");
        writer.write_all(b"# two").expect("other file");
        let bytes = writer.finish().expect("archive should finish").into_inner();
        let root = Some("repo-main");
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).expect("fixture archive");
        let destination =
            std::env::temp_dir().join(format!("android-tool-skill-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&destination);
        let extracted = extract_selected_skill(&mut archive, root, "skills/one", &destination)
            .expect("selected skill should extract");

        assert_eq!(extracted, 11);
        assert!(destination.join("SKILL.md").is_file());
        assert!(destination.join("README.md").is_file());
        assert!(!destination.join("two").exists());
        let _ = std::fs::remove_dir_all(destination);
    }

    #[test]
    fn imports_selected_skills_and_preserves_existing_skills_without_overwrite() {
        let destination =
            std::env::temp_dir().join(format!("skill-import-roundtrip-{}", std::process::id()));
        std::fs::create_dir_all(destination.join("remote-example")).unwrap();
        std::fs::write(
            destination.join("remote-example/SKILL.md"),
            "previous definition",
        )
        .unwrap();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("repo-main/remote/SKILL.md", options)
            .unwrap();
        writer
            .write_all(
                b"---\nname: remote-example\ndescription: imported docs\n---\nNew definition",
            )
            .unwrap();
        writer
            .start_file("repo-main/remote/helper.py", options)
            .unwrap();
        writer.write_all(b"print('helper')").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let repository = GitHubRepository {
            owner: "example".into(),
            name: "skills".into(),
            branch: "main".into(),
        };
        let selection = |resolution: &str| {
            vec![super::SkillImportSelection {
                source_path: "remote".into(),
                resolution: resolution.into(),
                renamed_id: None,
            }]
        };
        let skipped = super::import_skill_archive(
            repository.clone(),
            bytes.clone(),
            selection("keep"),
            destination.clone(),
        )
        .unwrap();
        assert!(skipped.imported_ids.is_empty());
        assert_eq!(skipped.skipped, vec!["remote-example"]);
        assert_eq!(
            std::fs::read_to_string(destination.join("remote-example/SKILL.md")).unwrap(),
            "previous definition"
        );
        let imported = super::import_skill_archive(
            repository,
            bytes,
            selection("overwrite"),
            destination.clone(),
        )
        .unwrap();
        assert_eq!(imported.imported_ids, vec!["remote-example"]);
        assert_eq!(imported.preview.destination, destination.to_string_lossy());
        assert!(
            std::fs::read_to_string(destination.join("remote-example/SKILL.md"))
                .unwrap()
                .contains("New definition")
        );
        assert!(destination.join("remote-example/helper.py").is_file());
        assert!(!destination.join(".remote-example.importing").exists());
        std::fs::remove_dir_all(destination).unwrap();
    }
}

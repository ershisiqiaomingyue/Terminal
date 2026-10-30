use super::*;

pub(super) fn import_destination(app: &AppHandle, requested: &str) -> Result<PathBuf, String> {
    let destination = if requested.trim().is_empty() {
        let path = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?
            .join("skills")
            .join("downloaded");
        fs::create_dir_all(&path).map_err(|error| format!("创建下载目录失败：{error}"))?;
        path
    } else {
        PathBuf::from(requested.trim())
    };
    if !destination.is_absolute() || !destination.is_dir() {
        return Err("Skills 下载目录必须是已存在的绝对路径".into());
    }
    let destination = destination
        .canonicalize()
        .map_err(|error| error.to_string())?;
    for builtin in builtin_skill_paths(app) {
        let builtin = builtin.canonicalize().map_err(|error| error.to_string())?;
        validate_import_destination(&destination, &builtin)?;
    }
    Ok(destination)
}

fn validate_import_destination(destination: &Path, builtin: &Path) -> Result<(), String> {
    if destination.starts_with(builtin) || builtin.starts_with(destination) {
        return Err("不能向内置技能目录或其父目录导入，请使用应用下载目录".into());
    }
    Ok(())
}

pub(super) fn validate_import_child(destination: &Path, target: &Path) -> Result<(), String> {
    if target.exists() {
        let resolved = target.canonicalize().map_err(|error| error.to_string())?;
        let root = destination
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !resolved.starts_with(&root) || resolved == root {
            return Err("技能导入目标越界".into());
        }
        if fs::symlink_metadata(target)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("不能覆盖符号链接技能目录".into());
        }
    }
    Ok(())
}

pub(super) fn skill_key(repository: &str, path: &str) -> String {
    serde_json::to_string(&(repository, path)).expect("string identity serialization")
}

pub(super) fn apply_skill_settings(skills: &mut [SkillView], settings: &StoredSkillSettings) {
    for skill in skills {
        skill.enabled = !settings
            .disabled_skills
            .contains(&skill_key(&skill.repository, &skill.path));
        if !skill.enabled {
            skill.status = "disabled".into();
            skill.status_message = "已停用，不参与自动选择或手动调用".into();
        }
    }
}

fn resolve_skill_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if Path::new(relative).is_absolute()
        || Path::new(relative).components().any(|part| {
            !matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("Skill 路径无效".into());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("无法定位技能目录：{error}"))?;
    let file = root
        .join(relative)
        .join("SKILL.md")
        .canonicalize()
        .map_err(|error| format!("无法定位技能说明：{error}"))?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err("Skill 路径越界或不是文件".into());
    }
    Ok(file)
}

fn resolve_managed_skill(
    app: &AppHandle,
    repository: &str,
    path: &str,
) -> Result<(PathBuf, bool), String> {
    let source = skill_sources(app, load_repositories(app)?)
        .into_iter()
        .find(|source| source.display_path == repository)
        .ok_or("技能目录未注册")?;
    let packages = scan_skill_repository(&source.path).map_err(|error| error.to_string())?;
    if !packages.iter().any(|package| package.path == path) {
        return Err("未找到该技能，请刷新列表".into());
    }
    let file = resolve_skill_file(&source.path, path)?;
    let packaged = builtin_skill_paths(app)
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| file.starts_with(root));
    Ok((file, source.builtin || packaged))
}

#[tauri::command]
pub async fn read_skill_details(
    app: AppHandle,
    repository: String,
    path: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (file, _) = resolve_managed_skill(&app, &repository, &path)?;
        let mut content = Vec::new();
        fs::File::open(file)
            .map_err(|error| error.to_string())?
            .take((MAX_SKILL_INSTRUCTION_BYTES + 1) as u64)
            .read_to_end(&mut content)
            .map_err(|error| error.to_string())?;
        if content.len() > MAX_SKILL_INSTRUCTION_BYTES {
            return Err("技能说明超过 128 KB 限制".into());
        }
        String::from_utf8(content).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn set_skill_enabled(
    app: AppHandle,
    repository: String,
    path: String,
    enabled: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        resolve_managed_skill(&app, &repository, &path)?;
        let _guard = SKILL_SETTINGS_LOCK
            .lock()
            .map_err(|_| "Skills 配置锁不可用")?;
        let mut settings = load_settings_unlocked(&app)?;
        let key = skill_key(&repository, &path);
        if enabled {
            settings.disabled_skills.remove(&key);
        } else {
            settings.disabled_skills.insert(key);
        }
        save_settings(&app, &settings)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn delete_skill(app: AppHandle, repository: String, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (file, builtin) = resolve_managed_skill(&app, &repository, &path)?;
        let _guard = SKILL_SETTINGS_LOCK
            .lock()
            .map_err(|_| "Skills 配置锁不可用")?;
        // Repositories may contain shared scripts or user files; remove only the registered definition.
        remove_skill_definition(&file, builtin)?;
        let mut settings = load_settings_unlocked(&app)?;
        settings
            .disabled_skills
            .remove(&skill_key(&repository, &path));
        save_settings(&app, &settings)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn remove_skill_definition(file: &Path, builtin: bool) -> Result<(), String> {
    if builtin {
        return Err("内置技能随应用提供，请使用停用开关".into());
    }
    fs::remove_file(file).map_err(|error| format!("删除技能失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_cannot_replace_builtin_definitions_or_their_parent() {
        let builtin = Path::new("/app/resources/agent-instructions/skills");
        assert!(validate_import_destination(builtin, builtin).is_err());
        assert!(validate_import_destination(&builtin.join("android-task-guidance"), builtin).is_err());
        assert!(validate_import_destination(Path::new("/app/resources"), builtin).is_err());
        assert!(validate_import_destination(Path::new("/data/skills/downloaded"), builtin).is_ok());
    }

    #[test]
    fn legacy_settings_and_disabled_identity_are_stable() {
        let settings: StoredSkillSettings = serde_json::from_str(r#"{"repositories":[]}"#).unwrap();
        assert!(settings.disabled_skills.is_empty());
        assert_ne!(skill_key("ab", "c"), skill_key("a", "bc"));
    }

    #[test]
    fn disabled_skills_do_not_remain_ready_in_the_catalog() {
        let mut skills = scan_catalog_with_python(
            vec![SkillRepositorySource {
                path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("resources/agent-instructions/skills"),
                display_path: "内置技能".into(),
                builtin: true,
            }],
            PythonRuntimeView {
                available: false,
                command: None,
                version: None,
                status: "not_checked".into(),
                message: String::new(),
            },
        )
        .skills;
        assert_eq!(skills.len(), 2);
        assert!(skills
            .iter()
            .all(|skill| skill.runtime == SkillRuntime::Instruction
                && skill.status == "ready"
                && !skill.description.is_empty()));
        let mut settings = StoredSkillSettings::default();
        settings
            .disabled_skills
            .insert(skill_key(&skills[0].repository, &skills[0].path));
        apply_skill_settings(&mut skills, &settings);
        assert!(!skills[0].enabled);
        assert_eq!(skills[0].status, "disabled");
        assert!(skills[1..]
            .iter()
            .all(|skill| skill.enabled && skill.status == "ready"));
    }

    #[test]
    fn rejects_traversal_and_accepts_root_skill_definition() {
        let root = std::env::temp_dir().join(format!(
            "skill-management-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), "root skill").unwrap();
        fs::write(root.join("helper.py"), "preserve scripts").unwrap();
        assert!(resolve_skill_file(&root, "../outside").is_err());
        assert!(resolve_skill_file(&root, &root.to_string_lossy()).is_err());
        assert_eq!(
            resolve_skill_file(&root, "").unwrap(),
            root.join("SKILL.md").canonicalize().unwrap()
        );
        assert!(remove_skill_definition(&root.join("SKILL.md"), true).is_err());
        assert!(root.join("SKILL.md").is_file());
        remove_skill_definition(&root.join("SKILL.md"), false).unwrap();
        assert!(root.join("helper.py").is_file());
        assert!(!root.join("SKILL.md").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

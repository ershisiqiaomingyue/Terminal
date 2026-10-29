use super::*;

fn instruction_resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/agent-instructions")
}

#[test]
fn instruction_upgrade_splits_only_the_legacy_default_and_keeps_a_backup() {
    let fixture = Fixture::new();
    let resources = instruction_resources();
    let legacy = fs::read_to_string(resources.join("legacy/AGENTS.v1.md")).unwrap();
    let root = fixture.0.join("user");
    fs::create_dir_all(&root).unwrap();
    // Match the previous Windows default, including its BOM and line endings.
    let original = format!("\u{feff}{}", legacy.replace("\r\n", "\n").replace('\n', "\r\n"));
    fs::write(root.join("AGENTS.md"), &original).unwrap();
    fs::write(root.join(".agent-instructions-v1.done"), "1\n").unwrap();
    initialize_agent_instructions(&root, &resources).unwrap();
    assert_eq!(fs::read_to_string(root.join("AGENTS.md.v1.bak")).unwrap(), original);
    let global = agent_core::load_global_agent_instructions(&root);
    assert_eq!(global.len(), 1);
    assert!(global[0].1.contains("默认使用简体中文"));
    assert!(global[0].1.contains("大肥鱼"));
    for domain in ["capture_android_logcat", "android_archive_edit", "Yocto", "AOSP"] {
        assert!(!global[0].1.contains(domain), "{domain}");
    }
    let skill = root.join("skills/guidance/android-task-guidance/SKILL.md");
    assert!(fs::read_to_string(&skill).unwrap().contains("capture_android_logcat"));
    // After the upgrade, editing/removal is persistent across initialization.
    fs::write(root.join("AGENTS.md"), "我的中文偏好").unwrap();
    fs::remove_file(&skill).unwrap();
    initialize_agent_instructions(&root, &resources).unwrap();
    assert_eq!(fs::read_to_string(root.join("AGENTS.md")).unwrap(), "我的中文偏好");
    assert!(!skill.exists());
}

#[test]
fn instruction_upgrade_preserves_custom_global_and_skill_files_and_deleted_globals() {
    let fixture = Fixture::new();
    let resources = instruction_resources();
    let legacy = fs::read_to_string(resources.join("legacy/AGENTS.v1.md")).unwrap();
    let custom = format!("{legacy}\n用户追加的规则\n");
    let root = fixture.0.join("custom");
    let skill = root.join("skills/guidance/android-task-guidance/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(root.join("AGENTS.md"), &custom).unwrap();
    fs::write(&skill, "用户自定义指导").unwrap();
    initialize_agent_instructions(&root, &resources).unwrap();
    assert_eq!(fs::read_to_string(root.join("AGENTS.md")).unwrap(), custom);
    assert_eq!(fs::read_to_string(skill).unwrap(), "用户自定义指导");
    assert!(!root.join("AGENTS.md.v1.bak").exists());

    let deleted = fixture.0.join("deleted");
    fs::create_dir_all(&deleted).unwrap();
    fs::write(deleted.join(".agent-instructions-v1.done"), "1\n").unwrap();
    initialize_agent_instructions(&deleted, &resources).unwrap();
    assert!(!deleted.join("AGENTS.md").exists());
    assert!(agent_core::load_global_agent_instructions(&deleted).is_empty());
}

#[test]
fn fresh_instruction_install_keeps_global_preferences_separate_from_optional_skills() {
    let fixture = Fixture::new();
    let root = fixture.0.join("new-user");
    let resources = instruction_resources();
    initialize_agent_instructions(&root, &resources).unwrap();
    initialize_android_tool_instructions(&root, &resources.join("ANDROID_TOOL.md")).unwrap();
    let globals = agent_core::load_global_agent_instructions(&root);
    assert_eq!(globals.len(), 2);
    assert!(globals[0].1.contains("默认使用简体中文"));
    assert!(globals[0].1.contains("大肥鱼"));
    assert!(globals[1].1.contains("Android Toolkit 专项指导"));
    assert!(!globals[0].1.contains("Android"));
    assert!(!globals[0].1.contains("Yocto"));
    assert_eq!(agent_core::scan_skill_repository(&root.join("skills/guidance")).unwrap().len(), 2);
    fs::remove_file(root.join("AGENTS.md")).unwrap();
    fs::remove_file(root.join("ANDROID_TOOL.md")).unwrap();
    initialize_agent_instructions(&root, &instruction_resources()).unwrap();
    initialize_android_tool_instructions(&root, &resources.join("ANDROID_TOOL.md")).unwrap();
    assert!(agent_core::load_global_agent_instructions(&root).is_empty());
}

#[test]
fn android_tool_guidance_is_seeded_once_and_user_edits_or_deletion_persist() {
    let fixture = Fixture::new();
    let resources = instruction_resources();
    let root = fixture.0.join("user");
    initialize_android_tool_instructions(&root, &resources.join("ANDROID_TOOL.md")).unwrap();
    let target = root.join("ANDROID_TOOL.md");
    assert!(fs::read_to_string(&target).unwrap().contains("Android Toolkit 专项指导"));
    fs::write(&target, "用户自定义产品指导").unwrap();
    initialize_android_tool_instructions(&root, &resources.join("ANDROID_TOOL.md")).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "用户自定义产品指导");
    fs::remove_file(&target).unwrap();
    initialize_android_tool_instructions(&root, &resources.join("ANDROID_TOOL.md")).unwrap();
    assert!(!target.exists());
}

#[test]
fn global_agent_rules_are_editable_seed_once_and_never_overwrite_user_files() {
    let fixture = Fixture::new();
    let source = fixture.0.join("default.md");
    fs::write(&source, "默认中文偏好").unwrap();
    let root = fixture.0.join("user");
    seed_agent_instructions(&root, &source).unwrap();
    assert_eq!(fs::read_to_string(root.join("AGENTS.md")).unwrap(), "默认中文偏好");
    fs::write(root.join("AGENTS.md"), "用户修改").unwrap();
    seed_agent_instructions(&root, &source).unwrap();
    assert_eq!(fs::read_to_string(root.join("AGENTS.md")).unwrap(), "用户修改");
    fs::remove_file(root.join("AGENTS.md")).unwrap();
    seed_agent_instructions(&root, &source).unwrap();
    assert!(!root.join("AGENTS.md").exists());
    let custom = fixture.0.join("custom");
    fs::create_dir_all(&custom).unwrap();
    fs::write(custom.join("agent.md"), "已有全局规则").unwrap();
    seed_agent_instructions(&custom, &source).unwrap();
    assert!(!custom.join("AGENTS.md").exists());
    assert_eq!(agent_core::load_global_agent_instructions(&custom).len(), 1);
}

struct Fixture(PathBuf);

#[test]
fn development_resources_are_editable_discoverable_and_seeded_once() {
    let fixture = Fixture::new();
    let root = fixture.0.join("user");
    let resources = instruction_resources().parent().unwrap().join("development");
    initialize_development_resources(&root, &resources).unwrap();
    let skills = agent_core::scan_skill_repository(&root.join("skills/development")).unwrap();
    assert_eq!(skills.len(),4);
    assert!(skills.iter().all(|skill| skill.runtime == agent_core::SkillRuntime::Instruction));
    let config = root.join("tools/lsp-servers.json");
    fs::write(&config,b"custom config").unwrap();
    let removed = root.join("skills/development/frontend-development/SKILL.md");
    fs::remove_file(&removed).unwrap();
    initialize_development_resources(&root, &resources).unwrap();
    assert_eq!(fs::read(&config).unwrap(),b"custom config");
    assert!(!removed.exists());
    assert!(!root.join("AGENTS.md").exists());
    let custom = fixture.0.join("custom");
    fs::create_dir_all(custom.join("tools")).unwrap();
    fs::write(custom.join("tools/lsp-servers.json"),"prior config").unwrap();
    initialize_development_resources(&custom, &resources).unwrap();
    assert_eq!(fs::read_to_string(custom.join("tools/lsp-servers.json")).unwrap(),"prior config");
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("android-user-storage-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn migration_preserves_originals_relocates_settings_and_never_resurrects_uninstalled_assets() {
    let fixture = Fixture::new();
    let root = fixture.0.join("new");
    let data = fixture.0.join("old-data");
    let config = fixture.0.join("old-config");
    let tools = fixture.0.join("old-tools");
    for directory in [data.join("model-plugins/workbuddy"), data.join("skills/downloaded/repo"), config.clone(), tools.clone(), root.join("plugins/workbuddy")] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::write(data.join("model-plugins/workbuddy/plugin.json"), b"old-plugin").unwrap();
    fs::write(root.join("plugins/workbuddy/plugin.json"), b"new-plugin").unwrap();
    fs::write(data.join("skills/downloaded/repo/SKILL.md"), b"skill").unwrap();
    let repository = data.join("skills/downloaded/repo").to_string_lossy().into_owned();
    let identity = serde_json::to_string(&(repository.clone(), "sample")).unwrap();
    fs::write(config.join("agent-skills.json"), serde_json::json!({"repositories":[repository,"D:/external-skills"],"disabled_skills":[identity]}).to_string()).unwrap();
    fs::write(tools.join("python.exe"), b"python").unwrap();
    fs::write(tools.join("environment-tools.json"), serde_json::json!({"paths":{"python":tools.join("python.exe").to_string_lossy(),"adb":"D:/Android/Sdk/adb.exe"}}).to_string()).unwrap();
    migrate(&root, &data, &config, Some(&tools)).unwrap();
    assert_eq!(fs::read(root.join("plugins/workbuddy/plugin.json")).unwrap(), b"new-plugin");
    assert_eq!(fs::read(data.join("model-plugins/workbuddy/plugin.json")).unwrap(), b"old-plugin");
    assert_eq!(fs::read(root.join("skills/downloaded/repo/SKILL.md")).unwrap(), b"skill");
    let settings: Value = serde_json::from_slice(&fs::read(root.join("skills/settings.json")).unwrap()).unwrap();
    assert_eq!(Path::new(settings["repositories"][0].as_str().unwrap()), root.join("skills/downloaded/repo"));
    assert_eq!(settings["repositories"][1], "D:/external-skills");
    let identity: (String, String) = serde_json::from_str(settings["disabled_skills"][0].as_str().unwrap()).unwrap();
    assert_eq!(Path::new(&identity.0), root.join("skills/downloaded/repo"));
    let environment: Value = serde_json::from_slice(&fs::read(root.join("tools/environment-tools.json")).unwrap()).unwrap();
    assert_eq!(Path::new(environment["paths"]["python"].as_str().unwrap()), root.join("tools/python.exe"));
    assert_eq!(environment["paths"]["adb"], "D:/Android/Sdk/adb.exe");
    fs::remove_file(root.join("plugins/workbuddy/plugin.json")).unwrap();
    migrate(&root, &data, &config, Some(&tools)).unwrap();
    assert!(!root.join("plugins/workbuddy/plugin.json").exists());
}

#[test]
fn bundles_are_versioned_and_missing_markers_are_not_loaded() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let cache = fixture.0.join("cache");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("tool.exe"), b"v1").unwrap();
    let fingerprint = directory_fingerprint(&source).unwrap();
    let first = cache_bundle(&source, &cache, &fingerprint).unwrap();
    fs::write(source.join("tool.exe"), b"v2").unwrap();
    let second = cache_bundle(&source, &cache, &directory_fingerprint(&source).unwrap()).unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(first.join("tool.exe")).unwrap(), b"v1");
    assert_eq!(fs::read(second.join("tool.exe")).unwrap(), b"v2");
    assert_eq!(active_bundle(&cache), Some(second.clone()));
    fs::remove_file(second.join(".complete")).unwrap();
    assert!(active_bundle(&cache).is_none());
    atomic_write(&cache.join("active-version.json"), br#""../outside""#).unwrap();
    assert!(active_bundle(&cache).is_none());
}

#[test]
fn atomic_write_updates_complete_files_and_path_mapping_checks_directory_boundaries() {
    let fixture = Fixture::new();
    let path = fixture.0.join("config.json");
    atomic_write(&path, b"first").unwrap();
    atomic_write(&path, b"second").unwrap();
    assert_eq!(fs::read(path).unwrap(), b"second");
    let mut value = serde_json::json!({"managed":"/old/tools/python.exe","external":"/old/tools-extra/adb.exe"});
    assert!(relocate_values(&mut value, &[(PathBuf::from("/old/tools"), PathBuf::from("/new/tools"))]));
    assert_eq!(Path::new(value["managed"].as_str().unwrap()), Path::new("/new/tools/python.exe"));
    assert_eq!(value["external"], "/old/tools-extra/adb.exe");
}

#[test]
fn legacy_appdata_cleanup_removes_only_a_pure_webview_cache_and_runs_once() {
    let fixture = Fixture::new();
    // 旧 identifier 目录里只有 WebView2 缓存：整目录删掉，并写下 marker。
    let root = fixture.0.join("user-cache");
    fs::create_dir_all(&root).unwrap();
    let local = fixture.0.join("local-cache");
    fs::create_dir_all(local.join("com.androidtool.app/EBWebView/Default")).unwrap();
    fs::write(local.join("com.androidtool.app/EBWebView/Default/Cache"), b"x").unwrap();
    cleanup_legacy_app_data(&root, Some(&local));
    assert!(!local.join("com.androidtool.app").exists());
    assert!(root.join(".legacy-appdata-cleanup-v1.done").is_file());

    // 目录里混进了用户自己的文件：整体跳过，一个字节都不动。
    let root = fixture.0.join("user-keep");
    fs::create_dir_all(&root).unwrap();
    let local = fixture.0.join("local-keep");
    fs::create_dir_all(local.join("com.androidtool.app/EBWebView")).unwrap();
    fs::write(local.join("com.androidtool.app/my-notes.txt"), b"keep").unwrap();
    cleanup_legacy_app_data(&root, Some(&local));
    assert!(local.join("com.androidtool.app/my-notes.txt").is_file());
}

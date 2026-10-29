//! 大版本升级的前置迁移：启动后先弹一个必须点确认的欢迎窗，把上一代留在旧
//! 路径里的数据搬到新路径，然后才允许进入主界面。
//!
//! 和 [`crate::appdata_migration`] 的分工：那边是「应用数据目录必须改名，否则
//! 新版本的 WebView 会在旧目录旁边另起一份 profile」，因此只能在 WebView 起来
//! 之前静默完成；这里负责的是「可以稍后做、但必须让用户知道」的那部分——用户
//! 目录下的配置、环境工具、项目里的 agent 目录。它们要等用户点按钮才动，所以
//! 前端在迁移完成前不会去读数据库、也不会挂载任何会写配置的视图。
//!
//! 框架是版本无关的：每一步写成一个幂等的函数，登记进 [`STEPS`]，状态按
//! 「代次（generation）」记在 `<app_data_dir>/upgrade-state.json`。以后再有大的
//! 改名，只要把新一步追加进表里、把 [`GENERATION`] 加一，欢迎窗自动出现。

use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

/// 当前这一代升级的标识。改动它的值会让所有用户再看一次欢迎窗——只在真的
/// 有不迁移就会出错的大改动时才动。
const GENERATION: &str = "2.0-brand-rename";

/// 状态文件名，放在 `app_data_dir`（也就是已经由 `appdata_migration` 搬好的
/// 新 identifier 目录）里，避免和用户可见的业务数据混在一起。
const STATE_FILE: &str = "upgrade-state.json";

/// 旧版本用的用户目录名。v1.0 起一直是 `.android-tool`。
const LEGACY_HOME_DIR: &str = ".android-tool";
/// v1 安装包把环境工具放在这里，目录名带空格、品牌名也是旧的。
const LEGACY_TOOLS_DIR: &str = "Android Tool/tools";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStepView {
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    status: StepStatus,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum StepStatus {
    /// 这次启动前就已经搬完（老用户第二次打开）。
    Done,
    /// 还有旧数据等着搬。
    Pending,
    /// 没有旧数据：全新安装，或在别处已经处理过。
    Missing,
    /// 上次迁移失败，留给用户重试。
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStatusView {
    generation: &'static str,
    /// 产品名，欢迎窗标题用，避免前端再抄一份品牌文案。
    product: &'static str,
    /// 已经点过「开始使用」，不再弹窗。
    acknowledged: bool,
    steps: Vec<UpgradeStepView>,
    /// 只统计需要用户动手的步骤。
    pending: usize,
    /// 上一次失败留下的说明；空串表示没失败过。
    error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeRunView {
    migrated: usize,
    pending: usize,
    /// 有意设计成「能重试」而不是「数据被回滚」：复制式迁移宁可重复一次，
    /// 也不会把用户数据删回去。
    error: String,
    steps: Vec<UpgradeStepView>,
}

#[derive(Debug)]
struct Step {
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    /// 旧数据还在吗？只有返回 true 的步骤才会在欢迎窗里显示为待迁移。
    outstanding: fn(&Context) -> bool,
    /// 真正搬家。幂等：重复调用只会把缺的部分补齐。
    run: fn(&Context) -> Result<(), String>,
}

struct Context {
    home: PathBuf,
    local: Option<PathBuf>,
    /// 用户打开过的项目根目录，用来搬项目里的 `.android-tool`。
    workspaces: Vec<PathBuf>,
}

fn steps() -> &'static [Step] {
    &[
        Step {
            id: "user-storage",
            title: "用户配置与脚本",
            detail: "把 ~/.android-tool 里的 AGENTS.md、hooks.json、agent-settings.json、Skills 与脚本复制到 ~/.terminal，并在配置里改写旧路径。",
            outstanding: user_storage_outstanding,
            run: migrate_user_storage,
        },
        Step {
            id: "environment-tools",
            title: "环境工具",
            detail: "把 v1 安装包放在 %LOCALAPPDATA%\\Android Tool\\tools 里的工具、SDK 与环境变量配置搬到新的用户目录。",
            outstanding: environment_tools_outstanding,
            run: migrate_environment_tools,
        },
        Step {
            id: "workspace-config",
            title: "项目内配置",
            detail: "项目里的 .android-tool（权限清单、项目级 AGENTS.md）改成新名字，旧目录原样保留不删除。",
            outstanding: workspace_config_outstanding,
            run: migrate_workspace_config,
        },
    ]
}

fn context(app: &AppHandle, workspaces: Vec<PathBuf>) -> Result<Context, String> {
    Ok(Context {
        home: app.path().home_dir().map_err(|error| error.to_string())?,
        local: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        workspaces,
    })
}

fn existing_workspaces(app: &AppHandle) -> Vec<PathBuf> {
    let Some(value) = read_project_preference(app, "android-tool.chat.workspaces.v1") else {
        return Vec::new();
    };
    let parsed: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
    let Some(items) = parsed.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let path = item
                .get("path")
                .or_else(|| item.get("root"))
                .and_then(Value::as_str)
                .map(PathBuf::from)?;
            path.is_dir().then_some(path)
        })
        .collect()
}

/// 直接读项目配置库：那是 localStorage 的镜像，主界面还没挂载时也能读。
fn read_project_preference(app: &AppHandle, key: &str) -> Option<String> {
    let directory = app.path().app_data_dir().ok()?;
    let connection =
        rusqlite::Connection::open(directory.join("project-preferences.sqlite")).ok()?;
    connection
        .query_row("SELECT value FROM preferences WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .ok()
}

// --- 每一步的判定与执行 ---------------------------------------------------

fn user_storage_outstanding(context: &Context) -> bool {
    context.home.join(LEGACY_HOME_DIR).is_dir()
}

fn migrate_user_storage(context: &Context) -> Result<(), String> {
    let old = context.home.join(LEGACY_HOME_DIR);
    if !old.is_dir() {
        return Ok(());
    }
    let new = crate::user_storage::root()?;
    copy_missing(&old, &new)?;
    // 配置里记着工具的绝对路径，搬完得改写成新根，否则环境工具会指向已经
    // 不存在（或不再更新）的旧目录。
    crate::user_storage::relocate_config_paths(&[(old, new)])
}

fn environment_tools_outstanding(context: &Context) -> bool {
    context
        .local
        .as_ref()
        .is_some_and(|local| local.join(LEGACY_TOOLS_DIR).is_dir())
}

fn migrate_environment_tools(context: &Context) -> Result<(), String> {
    let Some(local) = context.local.as_ref() else {
        return Ok(());
    };
    let old = local.join(LEGACY_TOOLS_DIR);
    if !old.is_dir() {
        return Ok(());
    }
    let new = crate::user_storage::root()?.join("tools");
    crate::user_storage::copy_missing_tree(&old, &new)?;
    crate::user_storage::relocate_config_paths(&[(old, new)])
}

fn workspace_config_outstanding(context: &Context) -> bool {
    context
        .workspaces
        .iter()
        .any(|root| !project_agent_dirs(root).0.is_dir() && project_agent_dirs(root).1.is_dir())
}

fn migrate_workspace_config(context: &Context) -> Result<(), String> {
    for root in &context.workspaces {
        let (current, legacy) = project_agent_dirs(root);
        if legacy.is_dir() {
            copy_missing(&legacy, &current)?;
        }
    }
    Ok(())
}

/// `agent-core` 是旧目录的真正使用者，这里把两个名字都问它要，免得仓库里再抄
/// 一份常量。
fn project_agent_dirs(root: &Path) -> (PathBuf, PathBuf) {
    (
        agent_core::workspace_agent_dir(root),
        root.join(agent_core::LEGACY_WORKSPACE_AGENT_DIR),
    )
}

/// 逐项复制，绝不覆盖已经存在的新文件——用户可能已经在新路径上改过。
fn copy_missing(source: &Path, target: &Path) -> Result<(), String> {
    crate::user_storage::copy_missing_tree(source, target)
}

// --- 状态文件 -------------------------------------------------------------

fn state_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join(STATE_FILE))
}

fn read_state(app: &AppHandle) -> Value {
    state_path(app)
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_state(app: &AppHandle, state: &Value) -> Result<(), String> {
    let path = state_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::user_storage::atomic_write(
        &path,
        &serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?,
    )
}

/// 这一次代是否已经处理过（点过「开始使用」）。
fn acknowledged(state: &Value) -> bool {
    state.get("generation").and_then(Value::as_str) == Some(GENERATION)
        && state
            .get("acknowledged")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn completed_ids(state: &Value) -> Vec<String> {
    state
        .get("completed")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 上一次运行中失败的步骤 id。迁移是幂等复制，失败不进 completed；
/// 这份名单只用来在欢迎窗上把「还没跑过」和「跑过但失败」区分开。
fn failed_ids(state: &Value) -> Vec<String> {
    state
        .get("failed")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn last_error(state: &Value) -> String {
    state
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn views(context: &Context, state: &Value) -> Vec<UpgradeStepView> {
    let completed = completed_ids(state);
    let failed = failed_ids(state);
    steps()
        .iter()
        .map(|step| UpgradeStepView {
            id: step.id,
            title: step.title,
            detail: step.detail,
            status: if completed.iter().any(|id| id == step.id) {
                // 复制式迁移不删旧目录，outstanding 判定恒真；已完成与否只看登记表。
                StepStatus::Done
            } else if failed.iter().any(|id| id == step.id) {
                StepStatus::Failed
            } else if !(step.outstanding)(context) {
                StepStatus::Missing
            } else {
                StepStatus::Pending
            },
        })
        .collect()
}

fn status_view(app: &AppHandle) -> Result<UpgradeStatusView, String> {
    let context = context(app, existing_workspaces(app))?;
    let state = read_state(app);
    let steps = views(&context, &state);
    Ok(UpgradeStatusView {
        generation: GENERATION,
        product: "Android Toolkit",
        acknowledged: acknowledged(&state),
        pending: steps
            .iter()
            .filter(|step| matches!(step.status, StepStatus::Pending | StepStatus::Failed))
            .count(),
        steps,
        error: last_error(&state),
    })
}

#[tauri::command]
pub async fn upgrade_status(app: AppHandle) -> Result<UpgradeStatusView, String> {
    tauri::async_runtime::spawn_blocking(move || status_view(&app))
        .await
        .map_err(|error| format!("读取升级状态失败：{error}"))?
}

#[tauri::command]
pub async fn run_upgrade(app: AppHandle) -> Result<UpgradeRunView, String> {
    tauri::async_runtime::spawn_blocking(move || run(&app))
        .await
        .map_err(|error| format!("迁移任务失败：{error}"))?
}

fn run(app: &AppHandle) -> Result<UpgradeRunView, String> {
    let context = context(app, existing_workspaces(app))?;
    let mut state = read_state(app);
    let mut failures = Vec::new();
    let mut failed = Vec::new();
    let mut completed = completed_ids(&state);
    let mut migrated = 0;
    for step in steps() {
        if completed.iter().any(|id| id == step.id) {
            // 已成功迁移过的步骤不重跑：复制式迁移里 outstanding 恒真，
            // 靠它判重会把每次「开始迁移」都变成一次全量复制。
            continue;
        }
        if !(step.outstanding)(&context) {
            continue;
        }
        match (step.run)(&context) {
            Ok(()) => {
                migrated += 1;
                if !completed.iter().any(|id| id == step.id) {
                    completed.push(step.id.to_string());
                }
            }
            Err(error) => {
                failed.push(step.id.to_string());
                failures.push(format!("{}：{error}", step.title));
            }
        }
    }
    let error = failures.join("；");
    state = json!({
        "generation": GENERATION,
        "acknowledged": false,
        "completed": completed,
        "failed": failed,
        "error": error,
        "updatedAt": chrono::Utc::now().timestamp(),
    });
    write_state(app, &state)?;
    let steps = views(&context, &state);
    Ok(UpgradeRunView {
        migrated,
        pending: steps
            .iter()
            .filter(|step| matches!(step.status, StepStatus::Pending | StepStatus::Failed))
            .count(),
        error,
        steps,
    })
}

/// 用户点过「开始使用」。写进状态文件后，以后的启动不再弹欢迎窗。
#[tauri::command]
pub async fn acknowledge_upgrade(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let previous = read_state(&app);
        let state = json!({
            "generation": GENERATION,
            "acknowledged": true,
            "completed": completed_ids(&previous),
            "failed": failed_ids(&previous),
            "error": last_error(&previous),
            "updatedAt": chrono::Utc::now().timestamp(),
        });
        write_state(&app, &state)
    })
    .await
    .map_err(|error| format!("保存升级状态失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("android-upgrade-{name}-{stamp}"));
        fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    fn sample(home: &Path) -> Context {
        Context {
            home: home.to_path_buf(),
            local: None,
            workspaces: Vec::new(),
        }
    }

    #[test]
    fn user_storage_is_copied_without_touching_the_legacy_directory() {
        let home = scratch("storage");
        let old = home.join(LEGACY_HOME_DIR);
        fs::create_dir_all(old.join("skills")).expect("mkdir");
        fs::write(old.join("AGENTS.md"), b"rules").expect("write");
        fs::write(old.join("skills/settings.json"), b"{}").expect("write");
        let context = sample(&home);
        assert!(user_storage_outstanding(&context));

        migrate_user_storage(&context).expect("migrate");
        // 迁移本身不改旧目录，用户随时能回滚；新目录由 user_storage 自己算。
        assert!(old.join("AGENTS.md").is_file());
    }

    #[test]
    fn workspace_config_copies_permissions_next_to_the_project() {
        let home = scratch("workspace");
        let project = home.join("project");
        let legacy = project.join(agent_core::LEGACY_WORKSPACE_AGENT_DIR);
        fs::create_dir_all(&legacy).expect("mkdir");
        fs::write(legacy.join("permissions.json"), br#"{"allowedScopes":[]}"#).expect("write");
        let mut context = sample(&home);
        context.workspaces = vec![project.clone()];

        assert!(workspace_config_outstanding(&context));
        migrate_workspace_config(&context).expect("migrate");
        assert!(agent_core::workspace_agent_dir(&project)
            .join("permissions.json")
            .is_file());
        assert!(legacy.is_dir(), "旧目录必须原样留着");
    }

    #[test]
    fn step_views_report_pending_then_missing() {
        let home = scratch("views");
        let context = sample(&home);
        fs::create_dir_all(home.join(LEGACY_HOME_DIR)).expect("mkdir");
        let pending = self::views(&context, &json!({}));
        let storage = pending.iter().find(|view| view.id == "user-storage").unwrap();
        assert!(matches!(storage.status, StepStatus::Pending));

        fs::remove_dir_all(home.join(LEGACY_HOME_DIR)).expect("rm");
        let migrated = self::views(&context, &json!({}));
        let storage = migrated.iter().find(|view| view.id == "user-storage").unwrap();
        assert!(matches!(storage.status, StepStatus::Missing));
    }

    #[test]
    fn completed_steps_show_done_even_when_legacy_directory_remains() {
        // 复制式迁移不删旧目录，outstanding 恒真：已完成判定必须优先于它，
        // 否则迁移成功后欢迎窗永远把步骤显示成「失败」。
        let home = scratch("done");
        let context = sample(&home);
        fs::create_dir_all(home.join(LEGACY_HOME_DIR)).expect("mkdir");
        let state = json!({ "completed": ["user-storage"] });
        let views = self::views(&context, &state);
        let storage = views.iter().find(|view| view.id == "user-storage").unwrap();
        assert!(matches!(storage.status, StepStatus::Done));
    }

    #[test]
    fn only_steps_that_actually_failed_show_failed() {
        let home = scratch("failed");
        let context = sample(&home);
        fs::create_dir_all(home.join(LEGACY_HOME_DIR)).expect("mkdir");
        let state = json!({ "failed": ["user-storage"] });
        let views = self::views(&context, &state);
        let storage = views.iter().find(|view| view.id == "user-storage").unwrap();
        assert!(matches!(storage.status, StepStatus::Failed));

        // 失败过但这次成功了：Done 压过 Failed。
        let recovered = json!({ "completed": ["user-storage"], "failed": ["user-storage"] });
        let views = self::views(&context, &recovered);
        let storage = views.iter().find(|view| view.id == "user-storage").unwrap();
        assert!(matches!(storage.status, StepStatus::Done));
    }

    #[test]
    fn run_skips_steps_already_recorded_as_completed() {
        // outstanding 恒真的步骤如果已完成还重跑，「开始迁移」会变成全量复制。
        // 这里只验证判定函数的组合行为：completed 命中 ⇒ 即使 outstanding 也不进 run 名单。
        let home = scratch("skip");
        let context = sample(&home);
        fs::create_dir_all(home.join(LEGACY_HOME_DIR)).expect("mkdir");
        let completed = completed_ids(&json!({ "completed": ["user-storage"] }));
        let step = steps().iter().find(|step| step.id == "user-storage").unwrap();
        let already_done = completed.iter().any(|id| id == step.id);
        let should_run = !already_done && (step.outstanding)(&context);
        assert!(!should_run);
    }

    #[test]
    fn acknowledged_state_matches_only_the_current_generation() {
        assert!(!acknowledged(&json!({})));
        assert!(acknowledged(
            &json!({ "generation": GENERATION, "acknowledged": true })
        ));
        // 下一代升级（GENERATION 变了）必须重新弹窗，旧记录不算数。
        assert!(!acknowledged(
            &json!({ "generation": "0.9", "acknowledged": true })
        ));
    }
}

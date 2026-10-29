use super::browser::{browser_command, browser_connection, validate_browser_file_path};
use super::*;
use crate::browser_mutations::{
    is_creation, is_move, mutation_name, BrowserMutationPreview, BrowserMutationRequest,
};

fn version_guard(inspect: &str, version: &str, message: &str) -> String {
    format!(
        "current=$({inspect}) || exit 1; [ \"$current\" = {} ] || {{ printf '%s\\n' {} >&2; exit 1; }}; ",
        shell_escape(version),
        shell_escape(message),
    )
}

/// 移动的探测分成两段：
/// - `inspect` 先打印两行类型（目标目录、源），再打印 `fingerprint` 的输出；
/// - `fingerprint` 是“目标是否存在 + 目录/源/同名项的 stat”，预览时的 version 就是它的输出，
///   执行前的 guard 也只重跑它。
/// 曾把整个 `inspect` 当作 guard 的命令（会多打印两行类型）与 `fields[2..]` 比对，
/// 永远不相等，于是 SSH 移动每次都报“源、目标目录或同名项已变化”。
fn move_inspection(directory_q: &str, path_q: &str, destination_q: &str) -> (String, String) {
    let fingerprint = format!("if [ -e {destination_q} ] || [ -L {destination_q} ]; then printf 'present\\n'; stat -c '%d:%i:%s:%y:%z:%f' -- {destination_q} || exit 1; else printf 'missing\\n'; fi; stat -Lc '%d:%i:%y:%z' -- {directory_q} || exit 1; stat -c '%d:%i:%s:%y:%z:%f' -- {path_q} || exit 1");
    // 目标目录用 -d（跟随链接）：软链接目录在文件树里就是按目录浏览的，
    // 移动进去也应当允许，只有指向非目录时才拒绝。
    let inspect = format!("if [ -d {directory_q} ]; then printf 'directory\\n'; else printf '移动目标不是文件夹\\n' >&2; exit 1; fi; if [ -L {path_q} ]; then printf 'link\\n'; elif [ -d {path_q} ]; then printf 'directory\\n'; elif [ -e {path_q} ]; then printf 'file\\n'; else printf '目标不存在\\n' >&2; exit 1; fi; {fingerprint}");
    (inspect, fingerprint)
}

fn mutation_path(request: &BrowserMutationRequest) -> Result<&str, String> {
    validate_browser_file_path(&request.root, &request.path)?;
    let path = request.path.as_str();
    if (!is_creation(request)
        && (path.trim_end_matches('/') == request.root.trim_end_matches('/')
            || path.ends_with('/')
            || path == "/"))
        || path.split('/').any(|part| part == ".")
    {
        return Err("禁止操作浏览根目录或含歧义的路径".into());
    }
    Ok(path)
}

fn remote_mutation_command(
    request: &BrowserMutationRequest,
    directory: bool,
    symlink: bool,
) -> Result<(String, Option<String>), String> {
    let path = mutation_path(request)?;
    let quoted = shell_escape(path);
    match request.action.as_str() {
        "create-file" | "create-directory" => {
            let name = mutation_name(request.new_name.as_deref())?;
            let destination = format!("{}/{name}", path.trim_end_matches('/'));
            let target = shell_escape(&destination);
            let operation = if request.action == "create-file" {
                format!("(set -C; : > {target})")
            } else {
                format!("mkdir -- {target}")
            };
            Ok((format!("if [ -e {target} ] || [ -L {target} ]; then printf '同名目标已存在，不覆盖\\n' >&2; exit 1; fi; {operation}"), Some(destination)))
        }
        "rename" => {
            let name = mutation_name(request.new_name.as_deref())?;
            let parent = path.rsplit_once('/').ok_or("没有父目录")?.0;
            let destination = format!("{parent}/{name}");
            // -T avoids moving into a concurrent directory; -n never clobbers.
            // Check afterward because GNU mv -n reports success when skipped.
            let target = shell_escape(&destination);
            Ok((format!("if [ -e {target} ] || [ -L {target} ]; then printf '同名目标已存在，不覆盖\\n' >&2; exit 1; fi; mv -nT -- {quoted} {target} || exit 1; if [ -e {quoted} ] || [ -L {quoted} ]; then printf '重命名未执行，目标可能已存在\\n' >&2; exit 1; fi"), Some(destination)))
        }
        "move" => {
            let destination_directory = request
                .destination_directory
                .as_deref()
                .ok_or("请选择移动目标文件夹")?;
            validate_browser_file_path(&request.root, destination_directory)?;
            let name = path
                .rsplit_once('/')
                .map(|(_, name)| name)
                .filter(|name| !name.is_empty())
                .ok_or("文件名为空")?;
            let destination = format!("{}/{name}", destination_directory.trim_end_matches('/'));
            let source_parent = path
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .filter(|parent| !parent.is_empty())
                .unwrap_or("/");
            if source_parent.trim_end_matches('/') == destination_directory.trim_end_matches('/') {
                return Err("文件已位于该文件夹".into());
            }
            let source_prefix = format!("{}/", path.trim_end_matches('/'));
            if directory
                && destination
                    .trim_end_matches('/')
                    .starts_with(&source_prefix)
            {
                return Err("不能将文件夹移动到自身的子目录".into());
            }
            let target = shell_escape(&destination);
            let exists = format!("if [ -e {target} ] || [ -L {target} ]; then");
            let overwrite = if request.overwrite {
                format!("rm -rf -- {target} || exit 1; mv -nT -- {quoted} {target} || exit 1;")
            } else {
                format!("printf '目标文件夹存在同名项，请确认是否覆盖\n' >&2; exit 1;")
            };
            Ok((format!("{exists} {overwrite} else mv -nT -- {quoted} {target} || exit 1; fi; if [ -e {quoted} ] || [ -L {quoted} ]; then printf '移动未执行，目标可能已存在\n' >&2; exit 1; fi"), Some(destination)))
        }
        "delete" if symlink || !directory => Ok((format!("rm -- {quoted}"), None)),
        "delete" if request.recursive => Ok((format!("rm -r -- {quoted}"), None)),
        "delete" => Ok((format!("rmdir -- {quoted}"), None)),
        _ => Err("未知文件操作".into()),
    }
}

pub fn preview_ssh_browser_mutation(
    config: &SshConnectionConfig,
    password: Option<&str>,
    request: &BrowserMutationRequest,
) -> Result<BrowserMutationPreview, String> {
    let config = browser_connection(config);
    // 带等待上限地占用泳道锁：以前这里是无上限的 lock()，同泳道上有慢操作时
    // 重命名/删除会一直卡住，界面上没有任何反馈。
    try_connection_operation(&config, browser_lock_wait(&config), || {
        preview_ssh_browser_mutation_locked(&config, password, request)
    })
}

fn preview_ssh_browser_mutation_locked(
    config: &SshConnectionConfig,
    password: Option<&str>,
    request: &BrowserMutationRequest,
) -> Result<BrowserMutationPreview, String> {
    let path = mutation_path(request)?;
    let parent = path.rsplit_once('/').ok_or("没有父目录")?.0;
    let parent = if parent.is_empty() { "/" } else { parent };
    let config = config;
    if is_creation(request) {
        let quoted = shell_escape(path);
        let version = browser_command(&config, password, &request.root,
            &format!("[ -d {quoted} ] || {{ printf '只能在文件夹内新建\n' >&2; exit 1; }}; stat -Lc '%d:%i:%y:%z' -- {quoted}"))?.trim_end_matches('\n').to_string();
        let (operation, destination) = remote_mutation_command(request, true, false)?;
        let inspect = format!("stat -Lc '%d:%i:%y:%z' -- {quoted}");
        let guard = version_guard(&inspect, &version, "父目录已变化，请重新确认");
        return Ok(BrowserMutationPreview {
            command: format!("{guard}{operation}"),
            version,
            destination,
            is_directory: request.action == "create-directory",
            is_symlink: false,
            conflict: false,
        });
    }
    if is_move(request) {
        let directory = request
            .destination_directory
            .as_deref()
            .ok_or("请选择移动目标文件夹")?;
        validate_browser_file_path(&request.root, directory)?;
        let name = path
            .rsplit_once('/')
            .map(|(_, name)| name)
            .filter(|name| !name.is_empty())
            .ok_or("文件名为空")?;
        let destination = format!("{}/{name}", directory.trim_end_matches('/'));
        let source_parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .filter(|parent| !parent.is_empty())
            .unwrap_or("/");
        if source_parent.trim_end_matches('/') == directory.trim_end_matches('/') {
            return Err("文件已位于该文件夹".into());
        }
        let source_prefix = format!("{}/", path.trim_end_matches('/'));
        if destination
            .trim_end_matches('/')
            .starts_with(&source_prefix)
        {
            return Err("不能将文件夹移动到自身的子目录".into());
        }
        let directory_q = shell_escape(directory);
        let path_q = shell_escape(path);
        let destination_q = shell_escape(&destination);
        let (inspect, fingerprint) = move_inspection(&directory_q, &path_q, &destination_q);
        let result = browser_command(&config, password, &request.root, &inspect)?;
        let fields = result.trim_end_matches('\n').lines().collect::<Vec<_>>();
        if fields.len() < 4 {
            return Err("移动预览信息不完整".into());
        }
        let _directory_kind = fields[0];
        let source_kind = fields[1];
        let conflict = fields[2] == "present";
        let version = fields[2..].join("\n");
        let is_directory = source_kind == "directory";
        let is_symlink = source_kind == "link";
        let (operation, destination) = remote_mutation_command(request, is_directory, is_symlink)?;
        let guard = version_guard(
            &fingerprint,
            &version,
            "源、目标目录或同名项已变化，请重新确认",
        );
        return Ok(BrowserMutationPreview {
            command: format!("{guard}{operation}"),
            version,
            destination,
            is_directory,
            is_symlink,
            conflict,
        });
    }
    // lstat the final entry: broken links are valid entries and never followed.
    let command = format!("if [ -L {path} ]; then printf 'link\n'; elif [ -d {path} ]; then printf 'directory\n'; elif [ -e {path} ]; then printf 'file\n'; else printf '目标不存在\n' >&2; exit 1; fi; stat -Lc '%d:%i' -- {parent} && stat -c '%d:%i:%s:%y:%z:%f' -- {path}", path=shell_escape(path), parent=shell_escape(parent));
    let result = browser_command(&config, password, &request.root, &command)?;
    let (kind, version) = result.split_once('\n').ok_or("目标信息不完整")?;
    let directory = kind == "directory";
    let symlink = kind == "link";
    let (operation, destination) = remote_mutation_command(request, directory, symlink)?;
    let version = version.trim_end_matches('\n').to_string();
    let inspect = format!(
        "stat -Lc '%d:%i' -- {parent_q} && stat -c '%d:%i:%s:%y:%z:%f' -- {path_q}",
        parent_q = shell_escape(parent),
        path_q = shell_escape(path)
    );
    let guard = version_guard(&inspect, &version, "目标或父目录已变化，请重新确认");
    Ok(BrowserMutationPreview {
        command: format!("{guard}{operation}"),
        version,
        destination,
        is_directory: directory,
        is_symlink: symlink,
        conflict: false,
    })
}
pub fn execute_ssh_browser_mutation(
    config: &SshConnectionConfig,
    password: Option<&str>,
    request: &BrowserMutationRequest,
) -> Result<BrowserMutationPreview, String> {
    if !request.acknowledged {
        return Err("需要先完成二次确认".into());
    }
    let preview = preview_ssh_browser_mutation(config, password, request)?;
    if request.expected_version.as_deref() != Some(preview.version.as_str()) {
        return Err("目标已变化，请重新确认操作".into());
    }
    let config = browser_connection(config);
    try_connection_operation(&config, browser_lock_wait(&config), || {
        browser_command(&config, password, &request.root, &preview.command)?;
        Ok(())
    })?;
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_keep_links_and_names_literal_and_recursion_explicit() {
        let mut request = BrowserMutationRequest {
            root: "/work".into(),
            path: "/work/a'; echo bad".into(),
            action: "delete".into(),
            new_name: None,
            recursive: false,
            expected_version: None,
            acknowledged: true,
            destination_directory: None,
            overwrite: false,
        };
        assert!(remote_mutation_command(&request, true, false)
            .unwrap()
            .0
            .starts_with("rmdir -- "));
        request.recursive = true;
        assert!(remote_mutation_command(&request, true, false)
            .unwrap()
            .0
            .starts_with("rm -r -- "));
        assert!(remote_mutation_command(&request, true, true)
            .unwrap()
            .0
            .starts_with("rm -- "));
        request.action = "rename".into();
        request.new_name = Some("-n $(touch hacked)".into());
        let command = remote_mutation_command(&request, false, false).unwrap().0;
        assert!(command.contains("mv -nT -- "));
        assert!(command.contains("'/work/-n $(touch hacked)'"));
        request.path = "/work".into();
        assert!(mutation_path(&request).is_err());
        request.path = "/work/../other".into();
        assert!(mutation_path(&request).is_err());
        request.path = "/other".into();
        assert!(mutation_path(&request).is_err());
        request.path = "/work".into();
        request.action = "create-file".into();
        request.new_name = Some("-n $(touch hacked)".into());
        let command = remote_mutation_command(&request, true, false).unwrap().0;
        assert!(command.contains("set -C"));
        assert!(command.contains("'/work/-n $(touch hacked)'"));
        request.action = "create-directory".into();
        assert!(remote_mutation_command(&request, true, false)
            .unwrap()
            .0
            .contains("mkdir -- "));
        request.new_name = Some("../escape".into());
        assert!(remote_mutation_command(&request, true, false).is_err());
        request.path = "/work/a.txt".into();
        request.action = "move".into();
        request.new_name = None;
        request.destination_directory = Some("/work/dst".into());
        request.overwrite = false;
        let command = remote_mutation_command(&request, false, false).unwrap().0;
        assert!(
            command.contains("mv -nT -- "),
            "normal SSH move uses no-clobber mv"
        );
        assert!(
            !command.contains("rm -rf -- "),
            "normal SSH move must not remove a target"
        );
        request.overwrite = true;
        let command = remote_mutation_command(&request, false, false).unwrap().0;
        assert!(
            command.contains("rm -rf -- "),
            "only explicit overwrite may remove a target"
        );
        assert!(
            command.contains("mv -nT -- "),
            "overwrite still uses no-clobber mv"
        );
        request.destination_directory = Some("/work/a.txt/child".into());
        request.overwrite = false;
        assert!(
            remote_mutation_command(&request, true, false).is_err(),
            "folder cannot move inside itself"
        );
    }

    #[test]
    fn move_guard_replays_only_the_fingerprint_not_the_type_prefix() {
        let (inspect, fingerprint) =
            move_inspection("'/work/dst'", "'/work/a.txt'", "'/work/dst/a.txt'");
        // inspect 多打印“目标目录/源”两行类型，fingerprint 不能包含它们，
        // 否则 version_guard 会把整段 inspect 的输出与 fields[2..] 比对，永不相等。
        assert!(inspect.starts_with("if [ -d '/work/dst' ]"));
        assert!(inspect.contains("printf 'link\\n'"));
        assert!(inspect.ends_with(&fingerprint));
        assert!(!fingerprint.contains("printf 'link\\n'"));
        assert!(!fingerprint.contains("printf 'directory\\n'"));
        // 目标目录用 -d（跟随软链接）而不是 -d && ! -L：软链接目录也应可作为移动目标。
        assert!(inspect.contains("if [ -d '/work/dst' ]; then printf 'directory\\n';"));
        assert!(
            !inspect.contains("! -L '/work/dst'"),
            "symlinked destination directories must stay movable"
        );
        // guard 只重跑 fingerprint，因此 $current 与预览时保存的 version 同源。
        let guard = version_guard(&fingerprint, "missing\n1:2:3:4:5\n6:7:8:9:a", "已变化");
        assert!(guard.contains(&fingerprint));
        assert!(!guard.contains("printf 'directory\\n'"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires ANDROID_DSH_TEST_WSL and an isolated SSH fixture"]
    fn real_ssh_browser_mutations() {
        let distro = std::env::var("ANDROID_DSH_TEST_WSL").expect("select test distro");
        let fixture = ssh_fixture::SshFixture::start(&distro);
        let config: SshConnectionConfig = serde_json::from_value(fixture.config.clone()).unwrap();
        let output = run_ssh_workspace_command(&config, None, &fixture.remote_root,
            "mkdir project; cd project; mkdir src; printf original > src/main.rs; printf existing > existing.rs; ln -s src linked",
            Arc::new(AtomicBool::new(false)), Duration::from_secs(15)).unwrap();
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        let root = format!("{}/project", fixture.remote_root);
        let mut request = BrowserMutationRequest {
            root: root.clone(),
            path: format!("{root}/src/main.rs"),
            action: "rename".into(),
            new_name: Some("renamed'; echo literal.rs".into()),
            recursive: false,
            expected_version: None,
            acknowledged: false,
            destination_directory: None,
            overwrite: false,
        };
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        assert!(execute_ssh_browser_mutation(&config, None, &request).is_err());
        request.acknowledged = true;
        let result = execute_ssh_browser_mutation(&config, None, &request).unwrap();
        let renamed = result.destination.unwrap();
        assert_eq!(
            read_ssh_browser_file(&config, None, &root, &renamed).unwrap(),
            "original"
        );
        request.path = format!("{root}/src");
        request.action = "delete".into();
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        assert!(execute_ssh_browser_mutation(&config, None, &request).is_err());
        request.path = format!("{root}/linked");
        request.recursive = true;
        let preview = preview_ssh_browser_mutation(&config, None, &request).unwrap();
        assert!(preview.is_symlink);
        assert!(preview.command.contains("rm -- "));
        request.expected_version = Some(preview.version);
        execute_ssh_browser_mutation(&config, None, &request).unwrap();
        assert_eq!(
            read_ssh_browser_file(&config, None, &root, &renamed).unwrap(),
            "original"
        );
        // A stale preview cannot delete an edited file.
        request.path = renamed.clone();
        request.recursive = false;
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        let output = run_ssh_workspace_command(
            &config,
            None,
            &root,
            &format!("printf changed-content > {}", shell_escape(&renamed)),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(15),
        )
        .unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(execute_ssh_browser_mutation(&config, None, &request).is_err());
        request.path = format!("{root}/src");
        request.recursive = true;
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        execute_ssh_browser_mutation(&config, None, &request).unwrap();
        // Exercise the OpenSSH fallback with a separate browser connection.
        remember_openssh_transport(&config);
        request.path = format!("{root}/existing.rs");
        request.recursive = false;
        request.action = "rename".into();
        request.new_name = Some("last.rs".into());
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        execute_ssh_browser_mutation(&config, None, &request).unwrap();
        request.path = root;
        request.root = fixture.remote_root.clone();
        request.action = "delete".into();
        request.recursive = true;
        let cleanup = request.clone();
        request.root = request.path.clone();
        request.action = "create-file".into();
        request.new_name = Some("new'; echo literal.txt".into());
        request.recursive = false;
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        let created = execute_ssh_browser_mutation(&config, None, &request)
            .unwrap()
            .destination
            .unwrap();
        assert_eq!(
            read_ssh_browser_file(&config, None, &request.root, &created).unwrap(),
            ""
        );
        let output = run_ssh_workspace_command(
            &config,
            None,
            &request.root,
            &format!("printf keep > {}", shell_escape(&created)),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(15),
        )
        .unwrap();
        assert_eq!(output.exit_code, 0);
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        assert!(execute_ssh_browser_mutation(&config, None, &request).is_err());
        assert_eq!(
            read_ssh_browser_file(&config, None, &request.root, &created).unwrap(),
            "keep"
        );
        request.new_name = Some("new folder".into());
        request.action = "create-directory".into();
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        execute_ssh_browser_mutation(&config, None, &request).unwrap();
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        assert!(execute_ssh_browser_mutation(&config, None, &request).is_err());
        request.new_name = Some("race.txt".into());
        request.action = "create-file".into();
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        let output = run_ssh_workspace_command(
            &config,
            None,
            &request.root,
            "printf concurrent > race.txt",
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(15),
        )
        .unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(execute_ssh_browser_mutation(&config, None, &request).is_err());
        assert_eq!(
            read_ssh_browser_file(
                &config,
                None,
                &request.root,
                &format!("{}/race.txt", request.root)
            )
            .unwrap(),
            "concurrent"
        );
        request = cleanup;
        request.expected_version = Some(
            preview_ssh_browser_mutation(&config, None, &request)
                .unwrap()
                .version,
        );
        execute_ssh_browser_mutation(&config, None, &request).unwrap();
    }
}

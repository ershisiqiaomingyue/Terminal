use super::{execute, LspOperation, LspParams};

#[test]
fn language_mappings_are_extensible_and_workspace_is_not_the_file_parent() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("lsp.json");
    std::fs::write(
        &config,
        include_str!("../../resources/development/lsp-servers.json"),
    )
    .unwrap();
    for (ext, id) in [
        ("rs", "rust"),
        ("cpp", "cpp"),
        ("py", "python"),
        ("ts", "typescript"),
        ("tsx", "typescriptreact"),
        ("js", "javascript"),
        ("go", "go"),
        ("java", "java"),
        ("swift", "swift"),
    ] {
        assert_eq!(
            configured_server(&config, &dir.path().join(format!("code.{ext}")))
                .unwrap()
                .1,
            id
        );
    }
    std::fs::write(&config,json!({"servers":[{"id":"custom","command":"custom-server","languages":{"custom":"new-language"},"filenames":{"BUILD":"build-language"}}]}).to_string()).unwrap();
    assert_eq!(
        configured_server(&config, Path::new("code.custom"))
            .unwrap()
            .1,
        "new-language"
    );
    assert_eq!(
        configured_server(&config, Path::new("BUILD")).unwrap().1,
        "build-language"
    );
    std::fs::create_dir_all(dir.path().join("deep/src")).unwrap();
    let file = dir.path().join("deep/src/code.custom");
    std::fs::write(&file, "").unwrap();
    assert_eq!(
        workspace_root(&file, &[dir.path().to_string_lossy().into_owned()]).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
    assert!(workspace_root(&file, &[]).is_err());
    assert!(find_executable("missing-lsp-fixture-483094")
        .unwrap_err()
        .contains("仍可使用源码检索"));
}

#[tokio::test]
async fn stdio_fixture_covers_language_root_configuration_push_pull_and_cancellation() {
    let python = find_executable("python").or_else(|_| find_executable("python3"));
    let Ok(python) = python else {
        eprintln!("Python fixture runtime unavailable");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script,r#"
import json,sys,time
def read():
    length=0
    while True:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line==b'\r\n': break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))
def send(value):
    data=json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(data)).encode()+data)
    sys.stdout.buffer.flush()
mode=sys.argv[1]
init=read()
root=init['params']['rootUri']
assert init['params']['workspaceFolders'][0]['uri']==root
send({'jsonrpc':'2.0','id':'configuration','method':'workspace/configuration','params':{'items':[{'section':'fixture.answer'}]}})
assert read()['result']==[42]
send({'jsonrpc':'2.0','id':init['id'],'result':{'capabilities':{'diagnosticProvider':{} if mode=='pull' else None}}})
assert read()['method']=='initialized'
assert read()['method']=='workspace/didChangeConfiguration'
send({'jsonrpc':'2.0','id':'folders','method':'workspace/workspaceFolders','params':{}})
languages={}
while True:
    message=read()
    method=message.get('method')
    if message.get('id')=='folders':
        assert message['result'][0]['uri']==root
    elif method=='textDocument/didOpen':
        document=message['params']['textDocument']
        assert document['uri'].startswith(root)
        languages[document['uri']]=document['languageId']
        if mode=='push':
            send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':document['uri'],'diagnostics':[{'message':document['languageId']}]}})
        elif mode=='wait':
            time.sleep(60)
    elif method in ('textDocument/diagnostic','textDocument/documentSymbol','textDocument/definition','textDocument/references'):
        if mode=='navigation':
            if method in ('textDocument/definition','textDocument/references'):
                assert message['params']['position']=={'line':2,'character':4}
        uri=message['params']['textDocument']['uri']
        send({'jsonrpc':'2.0','id':message['id'],'result':{'items':[{'message':languages[uri]}],'root':root}})
"#).unwrap();
    let config = dir.path().join("lsp.json");
    std::fs::create_dir_all(dir.path().join("nested/src")).unwrap();
    for (ext, language, mode) in [
        ("rs", "rust", "pull"),
        ("cpp", "cpp", "push"),
        ("py", "python", "pull"),
        ("go", "go", "push"),
        ("js", "javascript", "pull"),
        ("tsx", "typescriptreact", "push"),
    ] {
        std::fs::write(&config,json!({"servers":[{"id":"fixture","command":python,"args":[script,mode],"languages":{ext:language},"settings":{"fixture":{"answer":42}}}]}).to_string()).unwrap();
        let path = dir.path().join(format!("nested/src/code.{ext}"));
        std::fs::write(&path, "fixture source").unwrap();
        let result = execute(
            path,
            dir.path().to_owned(),
            config.clone(),
            LspOperation::Diagnostics,
            LspParams::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        assert_eq!(result["result"]["items"][0]["message"], language);
        assert_eq!(result["workspace"], dir.path().to_string_lossy().as_ref());
    }
    let path = dir.path().join("nested/src/navigation.js");
    std::fs::write(&path, "let x = 1").unwrap();
    std::fs::write(&config,json!({"servers":[{"id":"fixture","command":python,"args":[script,"navigation"],"languages":{"js":"javascript"},"settings":{"fixture":{"answer":42}}}]}).to_string()).unwrap();
    for operation in [
        LspOperation::Symbols,
        LspOperation::Definition,
        LspOperation::References,
    ] {
        let result = execute(
            path.clone(),
            dir.path().to_owned(),
            config.clone(),
            operation,
            LspParams::at(2, 4),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        assert_eq!(result["result"]["items"][0]["message"], "javascript");
    }
    // The navigation session above is resident. Expire it before the
    // cancellation-only fixture starts; otherwise the request path still
    // has no idle moment and the final sweep necessarily skips it.
    for session in test_sessions() {
        session.test_expire_idle();
    }
    reap_idle_sessions().await;
    assert_eq!(session_count(), 0);
    let path = dir.path().join("nested/src/wait.py");
    std::fs::write(&path, "").unwrap();
    std::fs::write(&config,json!({"servers":[{"id":"fixture","command":python,"args":[script,"wait"],"languages":{"py":"python"},"settings":{"fixture":{"answer":42}}}]}).to_string()).unwrap();
    let cancellation = Arc::new(AtomicBool::new(false));
    let token = cancellation.clone();
    let cancel = tokio::spawn(async move {
        sleep(Duration::from_millis(200)).await;
        token.store(true, Ordering::Relaxed);
    });
    let result = execute(
        path,
        dir.path().to_owned(),
        config,
        LspOperation::Diagnostics,
        LspParams::default(),
        cancellation,
    )
    .await;
    cancel.await.unwrap();
    assert!(result.unwrap_err().contains("取消"));
    // Resident sessions live in a process-global registry and outlive this
    // test body; kill them so no fixture process keeps the test binary alive.
    shutdown_sessions().await;
    assert_eq!(session_count(), 0);
}

#[test]
fn server_requests_cannot_write_files_or_change_permissions() {
    let response = client_response(
        &json!({"jsonrpc":"2.0","id":99,"method":"workspace/applyEdit","params":{"edit":{}}}),
        &Value::Null,
        "file:///workspace",
    )
    .unwrap();
    assert_eq!(response["error"]["code"], -32601);
}

/// Fixtures live under the crate's own `target/`: `%TEMP%` carries an ACL
/// restriction on some Windows setups and fails with 拒绝访问.
fn fixture_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("lsp-fixtures")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_fixture_executable(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"fixture").unwrap();
}

fn edit_of(id: &str, command: &str) -> LspServerEdit {
    LspServerEdit {
        id: id.to_string(),
        command: command.to_string(),
        args: vec![],
        languages: BTreeMap::from([("rs".to_string(), "rust".to_string())]),
    }
}

#[test]
fn executable_resolution_is_absolute_then_workspace_then_path() {
    let dir = fixture_dir("resolution-order");
    let workspace = dir.join("workspace");
    let bin = workspace.join("node_modules").join(".bin");
    let search = dir.join("path-like");
    write_fixture_executable(&bin.join("fixture-lsp"));
    write_fixture_executable(&search.join("fixture-lsp"));
    let absolute = dir.join("absolute-lsp");
    write_fixture_executable(&absolute);
    let project_dirs = project_executable_dirs(None, &workspace);
    let search_dirs = vec![search.clone()];

    // 绝对路径优先，并且解析结果就是配置里写的那一个。
    let resolved =
        resolve_executable(&absolute.to_string_lossy(), &project_dirs, &search_dirs).unwrap();
    assert_eq!(resolved.source, ExecutableSource::Absolute);
    assert_eq!(resolved.path, absolute);
    assert_eq!(resolved.source.label(), "绝对路径");
    assert!(resolved.launcher.is_none());

    // 其次是工作区本地 node_modules/.bin（PATH 里也有同名文件）。
    let resolved = resolve_executable("fixture-lsp", &project_dirs, &search_dirs).unwrap();
    assert_eq!(resolved.source, ExecutableSource::Workspace);
    assert_eq!(resolved.source.label(), "工作区 node_modules/.bin");
    assert!(
        resolved
            .path
            .ends_with(Path::new("node_modules").join(".bin").join("fixture-lsp")),
        "{}",
        resolved.path.display()
    );

    // 工作区里没有时才轮到 PATH。
    let resolved = resolve_executable("fixture-lsp", &[], &search_dirs).unwrap();
    assert_eq!(resolved.source, ExecutableSource::Path);
    assert_eq!(resolved.source.label(), "PATH");
    assert_eq!(resolved.path, search.join("fixture-lsp"));

    // 绝对路径写错时不回落到 PATH 里的同名文件：否则会悄悄换成另一个服务器。
    let wrong = dir.join("not-there-lsp");
    let error =
        resolve_executable(&wrong.to_string_lossy(), &project_dirs, &search_dirs).unwrap_err();
    assert!(error.contains("不存在"), "{error}");

    // 哪一层都没有：错误文本要说明查找顺序，并保留源码检索这条退路。
    let error =
        resolve_executable("missing-lsp-fixture-483094", &project_dirs, &search_dirs).unwrap_err();
    assert!(
        error.contains("查找顺序为 绝对路径 → 工作区 node_modules/.bin → PATH"),
        "{error}"
    );
    assert!(error.contains("仍可使用源码检索"), "{error}");
}

#[cfg(windows)]
#[test]
fn psscript_server_resolves_after_real_executables_and_gets_a_powershell_host() {
    let dir = fixture_dir("ps1-order");
    let workspace = dir.join("workspace");
    let bin = workspace.join("node_modules").join(".bin");
    write_fixture_executable(&bin.join("native-lsp"));
    write_fixture_executable(&bin.join("native-lsp.ps1"));
    write_fixture_executable(&bin.join("script-lsp.ps1"));
    let project_dirs = project_executable_dirs(None, &workspace);

    // `.ps1` 排在真实可执行文件之后：两个都在时优先原生文件。
    let resolved = resolve_executable("native-lsp", &project_dirs, &[]).unwrap();
    assert_eq!(
        resolved.path.extension(),
        None,
        "{}",
        resolved.path.display()
    );
    assert!(resolved.launcher.is_none());

    // 只有 `.ps1` 时也要能找到，并且带 PowerShell 主机前缀参数。
    let resolved = match resolve_executable("script-lsp", &project_dirs, &[]) {
        Ok(resolved) => resolved,
        Err(error) => {
            assert!(error.contains("PowerShell 主机"), "解析 .ps1 失败：{error}");
            eprintln!("本机没有 PowerShell 主机，跳过 .ps1 启动分支");
            return;
        }
    };
    assert_eq!(resolved.source, ExecutableSource::Workspace);
    assert!(
        resolved.path.ends_with("script-lsp.ps1"),
        "{}",
        resolved.path.display()
    );
    let launcher = resolved
        .launcher
        .expect(".ps1 必须通过 PowerShell 主机启动");
    assert!(launcher.program.is_file(), "{}", launcher.program.display());
    assert_eq!(
        &launcher.args[..5],
        [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass"
        ]
    );
    assert_eq!(launcher.args[5], "-File");
    assert!(
        launcher.args[6].ends_with("script-lsp.ps1"),
        "{}",
        launcher.args[6]
    );
}

#[test]
fn saving_one_server_keeps_unknown_fields_and_untouched_entries() {
    let dir = fixture_dir("save-keeps");
    let path = dir.join("lsp-servers.json");
    let original = json!({
        "$schema": "https://example.test/lsp.schema.json",
        "comment": "保持不变",
        "servers": [
            {"id": "rust", "command": "rust-analyzer", "args": [], "languages": {"rs": "rust"},
             "extra": {"vendor": "keep"}, "disabled": true},
            {"id": "python", "command": "pyright-langserver", "args": ["--stdio"],
             "languages": {"py": "python"}, "env": {"PYTHONPATH": "x"},
             "settings": {"python": {"analysis": true}}}
        ]
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();

    apply_lsp_server_edit(
        &path,
        &LspServerEdit {
            id: "rust".to_string(),
            command: "  custom-analyzer  ".to_string(),
            args: vec!["--stdio".to_string(), "--log".to_string()],
            languages: BTreeMap::from([
                ("rs".to_string(), "rust".to_string()),
                ("toml".to_string(), "toml".to_string()),
            ]),
        },
    )
    .unwrap();

    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["$schema"], original["$schema"],
        "未知的顶层字段必须保留"
    );
    assert_eq!(saved["comment"], original["comment"]);
    assert_eq!(
        saved["servers"][1], original["servers"][1],
        "未编辑的条目必须原样保留"
    );
    assert_eq!(
        saved["servers"][0]["extra"],
        json!({"vendor": "keep"}),
        "未知字段必须保留"
    );
    assert_eq!(
        saved["servers"][0]["disabled"],
        json!(true),
        "未编辑的已知字段也必须保留"
    );
    assert_eq!(saved["servers"][0]["command"], json!("custom-analyzer"));
    assert_eq!(saved["servers"][0]["args"], json!(["--stdio", "--log"]));
    assert_eq!(
        saved["servers"][0]["languages"],
        json!({"rs": "rust", "toml": "toml"})
    );
    assert_eq!(saved["servers"].as_array().unwrap().len(), 2);

    // 校验失败必须整体拒绝，且一个字节都不落盘。
    let before = std::fs::read(&path).unwrap();
    for edit in [
        LspServerEdit {
            id: "rust".to_string(),
            command: "   ".to_string(),
            args: vec![],
            languages: BTreeMap::from([("rs".to_string(), "rust".to_string())]),
        },
        LspServerEdit {
            id: "rust".to_string(),
            command: "rust-analyzer".to_string(),
            args: vec!["".to_string()],
            languages: BTreeMap::from([("rs".to_string(), "rust".to_string())]),
        },
        LspServerEdit {
            id: "rust".to_string(),
            command: "rust-analyzer".to_string(),
            args: vec![],
            languages: BTreeMap::new(),
        },
        LspServerEdit {
            id: "rust".to_string(),
            command: "rust-analyzer".to_string(),
            args: vec![],
            languages: BTreeMap::from([("src.rs".to_string(), "rust".to_string())]),
        },
        LspServerEdit {
            id: "unknown-server".to_string(),
            command: "somewhere".to_string(),
            args: vec![],
            languages: BTreeMap::from([("rs".to_string(), "rust".to_string())]),
        },
    ] {
        assert!(apply_lsp_server_edit(&path, &edit).is_err(), "{edit:?}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "校验失败不得改动文件"
        );
    }
}

#[test]
fn damaged_config_is_reported_and_never_overwritten() {
    let dir = fixture_dir("damaged");
    let path = dir.join("lsp-servers.json");
    let damaged = b"{ \"servers\": [ { \"id\": \"rust\" ".to_vec();
    std::fs::write(&path, &damaged).unwrap();

    let error = read_lsp_config_value(&path).unwrap_err();
    assert!(error.contains("不是有效 JSON"), "{error}");
    assert!(error.contains("原文件保持不变"), "{error}");
    // 探测视图本身不失败：问题装在视图里交给面板，面板才能一边显示文件名
    // 一边只在用户明确点「重置为默认」时覆盖。
    let view = lsp_config_view_at(&path, None).unwrap();
    assert_eq!(view.config_error.as_deref(), Some(error.as_str()));
    assert!(
        !view.using_bundled_default,
        "损坏时不得把内置默认当成用户配置"
    );
    assert!(view.servers.is_empty(), "损坏时不得列出任何服务器");
    assert!(apply_lsp_server_edit(
        &path,
        &LspServerEdit {
            id: "rust".to_string(),
            command: "rust-analyzer".to_string(),
            args: vec![],
            languages: BTreeMap::from([("rs".to_string(), "rust".to_string())]),
        }
    )
    .is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        damaged,
        "损坏的配置不得被默认配置覆盖"
    );

    // 结构合法但缺少 servers 数组同样拒绝，并且同样不覆盖。
    let wrong_shape = br#"{"version":2}"#.to_vec();
    std::fs::write(&path, &wrong_shape).unwrap();
    let error = read_lsp_config_value(&path).unwrap_err();
    assert!(error.contains("servers"), "{error}");
    let view = lsp_config_view_at(&path, None).unwrap();
    assert!(view
        .config_error
        .as_deref()
        .is_some_and(|message| message.contains("servers")));
    assert!(view.servers.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), wrong_shape);

    // 文件不存在是「尚未创建」，不是损坏：可以由内置默认值生成。
    assert!(read_lsp_config_value(&dir.join("missing.json"))
        .unwrap()
        .is_none());
}

#[test]
fn first_save_creates_the_file_from_the_bundled_default() {
    let dir = fixture_dir("first-save");
    let path = dir.join("tools").join("lsp-servers.json");
    let view = lsp_config_view_at(&path, None).unwrap();
    assert!(view.using_bundled_default);
    assert!(view.servers.len() >= 10);
    assert!(view.servers.iter().any(|server| server.id == "rust"));

    apply_lsp_server_edit(
        &path,
        &LspServerEdit {
            id: "rust".to_string(),
            command: "rust-analyzer-custom".to_string(),
            args: vec!["--stdio".to_string()],
            languages: BTreeMap::from([("rs".to_string(), "rust".to_string())]),
        },
    )
    .unwrap();

    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let servers = saved["servers"].as_array().unwrap();
    let rust = servers
        .iter()
        .find(|server| server["id"] == json!("rust"))
        .unwrap();
    assert_eq!(rust["command"], json!("rust-analyzer-custom"));
    assert_eq!(rust["args"], json!(["--stdio"]));
    assert!(
        servers.iter().any(|server| server["id"] == json!("python")),
        "未编辑的默认条目必须一起写入"
    );
    assert!(
        !lsp_config_view_at(&path, None)
            .unwrap()
            .using_bundled_default
    );
}

#[test]
fn server_probe_reports_status_resolved_path_and_unknown_fields() {
    let dir = fixture_dir("probe");
    let workspace = dir.join("workspace");
    write_fixture_executable(
        &workspace
            .join("node_modules")
            .join(".bin")
            .join("workspace-lsp"),
    );
    let path = dir.join("lsp-servers.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "servers": [
                {"id": "workspace-one", "command": "workspace-lsp", "args": [],
                 "languages": {"rs": "rust"}, "vendor": {"keep": true}},
                {"id": "disabled-one", "command": "workspace-lsp", "disabled": true,
                 "languages": {"rs": "rust"}},
                {"id": "missing-one", "command": "missing-lsp-fixture-483094",
                 "languages": {"rs": "rust"}},
                {"id": "broken-one", "args": ["--stdio"]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let workspace = workspace.to_string_lossy().into_owned();
    let view = lsp_config_view_at(&path, Some(workspace.as_str())).unwrap();
    assert!(!view.using_bundled_default);
    assert_eq!(view.config_path, display_path(&path));
    assert_eq!(view.servers.len(), 4);

    assert_eq!(view.servers[0].status, "ready");
    assert_eq!(view.servers[0].source, Some("工作区 node_modules/.bin"));
    assert_eq!(view.servers[0].extra_fields, vec!["vendor".to_string()]);
    let resolved = view.servers[0].resolved_path.clone().unwrap();
    assert!(
        resolved.ends_with(&format!(
            "node_modules{sep}.bin{sep}workspace-lsp",
            sep = std::path::MAIN_SEPARATOR
        )),
        "{resolved}"
    );
    assert!(
        !resolved.starts_with(r"\\?"),
        "面板不应展示 \\\\?\\ 前缀：{resolved}"
    );

    assert_eq!(view.servers[1].status, "disabled");
    assert!(
        view.servers[1].message.contains("禁用"),
        "{}",
        view.servers[1].message
    );
    assert!(view.servers[1].resolved_path.is_none());

    assert_eq!(view.servers[2].status, "missing");
    assert_eq!(view.servers[2].source, None);
    assert!(
        view.servers[2].message.contains("未安装或不在 PATH"),
        "{}",
        view.servers[2].message
    );

    // 结构无效的条目只影响它自己：其余服务器照常列出。
    assert_eq!(view.servers[3].status, "invalid");
    assert_eq!(view.servers[3].id, "broken-one");
    assert!(
        view.servers[3].message.contains("无效"),
        "{}",
        view.servers[3].message
    );

    // 工作区目录写错时列表整体失败，而不是静默忽略本地 bin。
    let bad_workspace = dir.join("no-such-workspace").to_string_lossy().into_owned();
    let error = lsp_config_view_at(&path, Some(bad_workspace.as_str())).unwrap_err();
    assert!(error.contains("工作区目录不存在"), "{error}");
}

#[test]
fn duplicate_ids_and_empty_commands_are_refused_before_writing() {
    let dir = fixture_dir("write-validation");
    let path = dir.join("lsp-servers.json");
    let write = |value: &Value| {
        std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        std::fs::read(&path).unwrap()
    };

    // id 重复：编辑按 id 定位条目，重复会让保存落到哪一条上变得不确定。
    let before = write(&json!({
        "servers": [
            {"id": "rust", "command": "rust-analyzer", "languages": {"rs": "rust"}},
            {"id": "Rust", "command": "other-analyzer", "languages": {"rs": "rust"}}
        ]
    }));
    let error = apply_lsp_server_edit(&path, &edit_of("rust", "rust-analyzer")).unwrap_err();
    assert!(error.contains("重复"), "{error}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "重复 id 的文档不得被写回"
    );

    // 启用中的条目缺少 command：整体拒绝，并指出是哪一条。
    let before = write(&json!({
        "servers": [
            {"id": "rust", "command": "rust-analyzer", "languages": {"rs": "rust"}},
            {"id": "python", "args": ["--stdio"], "languages": {"py": "python"}}
        ]
    }));
    let error = apply_lsp_server_edit(&path, &edit_of("rust", "rust-analyzer")).unwrap_err();
    assert!(
        error.contains("python") && error.contains("缺少 command"),
        "{error}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // 条目缺少 id：同样拒绝，否则用户看不出该改哪一项。
    let before = write(&json!({
        "servers": [
            {"id": "rust", "command": "rust-analyzer", "languages": {"rs": "rust"}},
            {"command": "nameless-lsp", "languages": {"rs": "rust"}}
        ]
    }));
    let error = apply_lsp_server_edit(&path, &edit_of("rust", "rust-analyzer")).unwrap_err();
    assert!(error.contains("缺少非空 id"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // 已禁用的条目永远不会被启动，允许它没有 command：无关的保存不该被挡住。
    write(&json!({
        "servers": [
            {"id": "rust", "command": "rust-analyzer", "languages": {"rs": "rust"}},
            {"id": "python", "disabled": true, "languages": {"py": "python"}}
        ]
    }));
    apply_lsp_server_edit(&path, &edit_of("rust", "custom-analyzer")).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["servers"][0]["command"], json!("custom-analyzer"));
    assert_eq!(saved["servers"][1]["disabled"], json!(true));
}

/// 「找到但无法启动」与「未安装」必须给出不同的 status。本机总能找到
/// PowerShell（`pwsh` 或系统自带的 powershell.exe），真实探测走不到这条
/// 分支，所以这里直接断言状态映射本身。
#[test]
fn unlaunchable_servers_are_reported_apart_from_missing_ones() {
    let missing = executable_missing("fixture-lsp", false);
    assert_eq!(missing.status(), "missing");
    assert!(!missing.unlaunchable);
    assert!(missing.contains("查找顺序为"), "{missing}");

    let unlaunchable = ResolveFailure::unlaunchable("系统中未找到 PowerShell 主机".to_string());
    assert_eq!(unlaunchable.status(), "unlaunchable");
    assert!(unlaunchable.unlaunchable);
    assert_eq!(unlaunchable.to_string(), "系统中未找到 PowerShell 主机");
    assert_eq!(
        String::from(unlaunchable),
        "系统中未找到 PowerShell 主机",
        "`?` 必须能把它当成 String 返回"
    );
}

#[test]
fn damaged_config_is_replaced_only_by_an_explicit_reset() {
    let dir = fixture_dir("reset");
    let path = dir.join("lsp-servers.json");
    let damaged = b"{ \"servers\": [ { \"id\": \"rust\" ".to_vec();
    std::fs::write(&path, &damaged).unwrap();

    // 读取与保存都不会碰它（详细断言见 damaged_config_is_reported_and_never_overwritten）。
    let view = lsp_config_view_at(&path, None).unwrap();
    assert!(view.config_error.is_some());
    assert!(view.servers.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), damaged);

    // 只有显式重置才覆盖，并且写回的就是内置默认配置。
    let view = reset_lsp_config_at(&path, None).unwrap();
    let bundled: Value = serde_json::from_str(BUNDLED_LSP_CONFIG).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_ne!(std::fs::read(&path).unwrap(), damaged);
    assert_eq!(saved, bundled, "重置必须写回内置默认配置");
    assert_eq!(
        view.servers.len(),
        bundled["servers"].as_array().unwrap().len()
    );
    assert!(!view.using_bundled_default, "重置后配置来自用户文件");
    assert_eq!(view.config_path, display_path(&path));
    assert!(view.servers.iter().any(|server| server.id == "rust"));
}

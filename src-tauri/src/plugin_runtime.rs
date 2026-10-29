//! The one install/launch pipeline every plugin capability shares:
//! resolve runtime -> download -> verify -> launch.
//!
//! Model provider adapters and host plugins used to keep separate copies of the
//! same mechanics, and each copy named its own interpreter ("node") in its own
//! source. Here the interpreter comes from the plugin's own declaration
//! (`androidTool.requiredEnvironmentTools`), so a plugin that declares Python
//! takes the pip path while a Node plugin takes the npm path, without any
//! caller branching on the runtime id.

use crate::plugin_environments::PluginEnvironments;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Interpreter a plugin runs on. The ids are the vocabulary shared by plugin
/// manifests, the environment UI, and `PluginEnvironments`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    Node,
    Python,
}

impl Language {
    fn id(self) -> &'static str {
        match self {
            Language::Node => "node",
            Language::Python => "python",
        }
    }

    fn declared(id: &str) -> Result<Self, String> {
        match id {
            "node" => Ok(Language::Node),
            "python" => Ok(Language::Python),
            other => Err(format!("不支持的插件运行环境：{other}")),
        }
    }

    /// A plugin runs on exactly one interpreter: the first one it declares.
    /// A plugin that declares nothing keeps the historical Node default.
    pub(crate) fn primary(required: &[&str]) -> Result<Self, String> {
        match required.first() {
            Some(id) => Self::declared(id),
            None => Ok(Language::Node),
        }
    }
}

/// Why a pipeline step stopped. Callers keep their own user-facing wording, so
/// the failure is handed back structured instead of pre-formatted.
#[derive(Debug)]
pub(crate) enum Failure {
    Spawn(String),
    TimedOut,
    Exit { tail: String },
}

/// How a package manager installs into a private plugin directory.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InstallOptions {
    /// `--install-links`: pack local sources instead of symlinking them. Only
    /// the host plugin path can install from a directory or an archive.
    pub local_links: bool,
    /// Report download progress as json inside the plugin directory.
    pub observe: bool,
}

/// Resolved runtime plus the language-specific package manager and entry point
/// conventions.
pub(crate) struct Pipeline {
    environments: PluginEnvironments,
    language: Language,
}

impl Pipeline {
    /// Resolves the runtimes a plugin declared. `legacy_node` lets an older
    /// privately installed Node runtime be adopted instead of re-downloaded.
    pub(crate) fn resolve(required: &[&str], legacy_node: Option<&Path>) -> Result<Self, String> {
        let language = Language::primary(required)?;
        Ok(Self {
            environments: PluginEnvironments::resolve(required, legacy_node)?,
            language,
        })
    }

    pub(crate) fn language(&self) -> Language {
        self.language
    }

    fn executable(&self) -> Result<&Path, String> {
        self.environments.executable(self.language.id())
    }

    /// Runs the plugin's interpreter with no arguments yet.
    fn command(&self) -> Result<Command, String> {
        self.environments.command(self.language.id())
    }

    /// Everything that follows the interpreter on a Node install, in the order
    /// Node itself requires: Node options are read from the command line before
    /// the entry script, so `--require` has to precede the npm entry script.
    /// Appending it after made npm treat the observer path as a command name and
    /// fail every observed install with `Unknown command`.
    fn node_command_line(
        observer: Option<&Path>,
        npm: &Path,
        local_links: bool,
    ) -> Vec<std::ffi::OsString> {
        let mut arguments: Vec<std::ffi::OsString> = Vec::new();
        if let Some(observer) = observer {
            arguments.push("--require".into());
            arguments.push(observer.into());
        }
        arguments.push(npm.into());
        // No `--legacy-peer-deps`: it makes npm ignore every peerDependencies
        // declaration, so a peer the host list happens to omit only shows up as
        // an import failure later (dsh-opencode-go declares twenty peers while
        // the host list named ten; dsh-pet needed @deepseek-ai/dsh-home-paths).
        // Every plugin owns a private dependency graph now, so npm's own peer
        // resolution is what keeps an install complete.
        arguments.extend(
            [
                "install",
                "--ignore-scripts",
                "--omit=dev",
                "--no-audit",
                "--no-fund",
                "--save-exact",
                // Keeps an install usable on a poor connection. Side effect worth
                // knowing: a warm npm cache serves every tarball locally, so the
                // progress observer reports zero bytes and the bar sits still even
                // though the install succeeded.
                "--prefer-offline",
                "--registry=https://registry.npmjs.org",
            ]
            .map(std::ffi::OsString::from),
        );
        if local_links {
            arguments.push("--install-links".into());
        }
        arguments
    }

    /// Command that installs `packages` into `directory` itself, so the plugin
    /// owns its whole dependency graph instead of sharing one with the host.
    pub(crate) fn install_command(
        &self,
        directory: &Path,
        packages: &[String],
        options: InstallOptions,
    ) -> Result<Command, String> {
        let mut command = self.command()?;
        match self.language {
            Language::Node => {
                let npm = crate::android_tools::node_npm_cli(self.executable()?)
                    .ok_or("找不到插件安装所需的 npm")?;
                let observer = if options.observe {
                    let observer = directory.join("download-progress.cjs");
                    fs::write(&observer, include_str!("plugin_host/download-progress.cjs"))
                        .map_err(|error| error.to_string())?;
                    // Applies to this npm invocation only, never to plugin code.
                    command.env(
                        "ANDROID_TOOL_NPM_PROGRESS",
                        directory.join("download-progress.json"),
                    );
                    Some(observer)
                } else {
                    None
                };
                command.args(Self::node_command_line(
                    observer.as_deref(),
                    &npm,
                    options.local_links,
                ));
            }
            Language::Python => {
                command.args([
                    "-m",
                    "pip",
                    "install",
                    "--no-input",
                    "--disable-pip-version-check",
                    "--progress-bar",
                    "off",
                    "--target",
                ]);
                command.arg(directory);
            }
        }
        command.args(packages).current_dir(directory);
        Ok(command)
    }

    /// Command that proves the installed package really exposes the API the
    /// adapter was written against, using the interpreter that will run it.
    pub(crate) fn verification_command(
        &self,
        directory: &Path,
        package: &str,
        exports: &[&str],
    ) -> Result<Command, String> {
        let mut command = self.command()?;
        match self.language {
            Language::Node => {
                let validation = format!(
                    "const m = await import({}); for (const key of {}) {{ if (!m[key]) throw new Error('missing '+key); }}",
                    serde_json::to_string(package).map_err(|error| error.to_string())?,
                    serde_json::to_string(exports).map_err(|error| error.to_string())?,
                );
                command.args(["--input-type=module", "-e", &validation]);
            }
            Language::Python => {
                // JSON string and array literals are also valid Python literals.
                let validation = format!(
                    "import importlib\nmodule = importlib.import_module({})\nfor key in {}:\n    if not hasattr(module, key):\n        raise SystemExit('missing ' + key)\n",
                    serde_json::to_string(package).map_err(|error| error.to_string())?,
                    serde_json::to_string(exports).map_err(|error| error.to_string())?,
                );
                command.args(["-c", &validation]);
            }
        }
        command.current_dir(directory);
        Ok(command)
    }

    /// Command that starts an installed entry point. Callers append their own
    /// arguments and environment on top of it.
    pub(crate) fn launch_command(
        &self,
        directory: &Path,
        entry: &Path,
    ) -> Result<Command, String> {
        let mut command = self.command()?;
        command.arg(entry);
        if self.language == Language::Python {
            // `pip install --target` puts the plugin's own packages here.
            command.env("PYTHONPATH", directory);
        }
        command.current_dir(directory);
        Ok(command)
    }
}

/// Private version directory. The plugin owns its dependency graph, and a
/// failed install can be deleted without touching the previous version.
pub(crate) struct InstallDir {
    pub(crate) directory: PathBuf,
}

pub(crate) fn prepare_private_dir(
    root: &Path,
    stamp: &str,
    language: Language,
) -> Result<InstallDir, String> {
    let directory = root.join("versions").join(stamp);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    if language == Language::Node {
        // npm installs the plugin's dependencies next to this manifest, and the
        // module type keeps an ESM entry point loadable from that directory.
        fs::write(directory.join("package.json"), PRIVATE_MANIFEST)
            .map_err(|error| error.to_string())?;
    }
    Ok(InstallDir { directory })
}

const PRIVATE_MANIFEST: &str = "{\"private\":true,\"type\":\"module\"}";

/// Proves the package manager really put the plugin in the version directory,
/// so a half-downloaded version is rejected before it is started.
pub(crate) fn is_installed(language: Language, directory: &Path, package: &str) -> bool {
    match language {
        // npm keeps the plugin under node_modules beside the private manifest.
        Language::Node => directory
            .join("node_modules")
            .join(package)
            .join("package.json")
            .is_file(),
        // pip --target lays packages down directly; the import check is what
        // proves they are usable, so the directory only has to exist.
        Language::Python => directory.is_dir(),
    }
}

/// Runs one pipeline command with its output captured in the plugin directory.
/// Every step shares the same timeout/kill/log-tail behaviour so a stuck
/// download can never leave the UI waiting forever.
pub(crate) fn run(
    mut command: Command,
    directory: &Path,
    timeout: Duration,
    tail_lines: usize,
    mut tick: impl FnMut(&Path, bool),
) -> Result<(), Failure> {
    let log_path = directory.join("install.log");
    let log =
        fs::File::create(&log_path).map_err(|error| Failure::Spawn(error.to_string()))?;
    command
        .stdout(Stdio::from(
            log.try_clone()
                .map_err(|error| Failure::Spawn(error.to_string()))?,
        ))
        .stderr(Stdio::from(log));
    let mut child = command
        .spawn()
        .map_err(|error| Failure::Spawn(error.to_string()))?;
    let started = Instant::now();
    loop {
        tick(directory, false);
        if let Some(status) = child
            .try_wait()
            .map_err(|error| Failure::Spawn(error.to_string()))?
        {
            tick(directory, true);
            if status.success() {
                return Ok(());
            }
            return Err(Failure::Exit {
                tail: log_tail(&log_path, tail_lines),
            });
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn log_tail(path: &Path, lines: usize) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// A half-installed version directory is never reused.
pub(crate) fn remove_failed(directory: &Path) {
    let _ = fs::remove_dir_all(directory);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "android-tool-plugin-runtime-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn primary_language_follows_the_plugin_declaration() {
        assert_eq!(Language::primary(&["python"]).unwrap(), Language::Python);
        assert_eq!(Language::primary(&["node"]).unwrap(), Language::Node);
        assert_eq!(Language::primary(&[]).unwrap(), Language::Node);
        // Declaring a tool after the interpreter does not change the runtime.
        assert_eq!(
            Language::primary(&["python", "node"]).unwrap(),
            Language::Python
        );
        let error = Language::primary(&["ruby"]).unwrap_err();
        assert!(error.contains("ruby"), "{error}");
    }

    #[test]
    fn private_directory_owns_its_manifest() {
        let root = scratch("manifest");
        let install = prepare_private_dir(&root, "1700000000000", Language::Node).unwrap();
        assert_eq!(install.directory, root.join("versions").join("1700000000000"));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(install.directory.join("package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["private"], true);
        assert_eq!(manifest["type"], "module");
        // Re-preparing the same version keeps the directory usable.
        prepare_private_dir(&root, "1700000000000", Language::Node).unwrap();
        // A Python plugin is laid out by pip, so it gets no npm manifest.
        let python = prepare_private_dir(&root, "1700000000001", Language::Python).unwrap();
        assert!(python.directory.is_dir());
        assert!(!python.directory.join("package.json").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installed_layout_is_language_specific() {
        let root = scratch("installed");
        let directory = prepare_private_dir(&root, "1700000000002", Language::Node)
            .unwrap()
            .directory;
        // A version directory without the package is not installed.
        assert!(!is_installed(Language::Node, &directory, "dsh-opencode-go"));
        let package = directory.join("node_modules").join("dsh-opencode-go");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("package.json"), "{}").unwrap();
        assert!(is_installed(Language::Node, &directory, "dsh-opencode-go"));
        // pip --target owns the layout, so only the directory has to exist.
        assert!(is_installed(Language::Python, &directory, "dsh-opencode-go"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn node_options_precede_the_npm_entry_script() {
        let npm = Path::new("/usr/lib/node_modules/npm/bin/npm-cli.js");
        let observer = Path::new("/plugins/versions/1/download-progress.cjs");
        let line = Pipeline::node_command_line(Some(observer), npm, true);
        let position = |needle: &str| {
            line.iter()
                .position(|argument| argument.to_string_lossy() == needle)
                .unwrap_or_else(|| panic!("{needle} missing from {line:?}"))
        };
        let npm_position = line
            .iter()
            .position(|argument| argument.to_string_lossy() == npm.to_string_lossy())
            .expect("the npm entry script is missing");
        // A node option placed after the entry script is read by npm instead, which
        // then sees the observer path as a command name and fails the whole install.
        assert_eq!(position("--require"), 0, "{line:?}");
        assert!(npm_position > position("--require"), "{line:?}");
        assert_eq!(position("install"), npm_position + 1, "{line:?}");
        assert!(position("--registry=https://registry.npmjs.org") > position("install"));
        assert!(position("--install-links") > position("--registry=https://registry.npmjs.org"));
        // Without progress reporting nothing is inserted before the entry script.
        let plain = Pipeline::node_command_line(None, npm, false);
        assert_eq!(
            plain.first().map(|argument| argument.to_string_lossy().into_owned()),
            Some(npm.to_string_lossy().into_owned())
        );
        assert!(!plain
            .iter()
            .any(|argument| argument.to_string_lossy() == "--require"));
    }

    /// Manual check that a real registry install both finishes and reports
    /// progress. Run with `cargo test --lib network_install -- --ignored --nocapture`
    /// (needs network and a local Node runtime, so it stays out of the suite).
    #[test]
    #[ignore = "needs network and a local Node runtime"]
    fn network_install_reports_download_progress() {
        let pipeline = Pipeline::resolve(&["node"], None).expect("node runtime");
        let directory = std::env::temp_dir().join(format!(
            "plugin-runtime-network-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        fs::create_dir_all(&directory).unwrap();
        // A cold cache makes the byte counter deterministic. With a warm npm cache
        // `--prefer-offline` serves every tarball locally and npm reports zero
        // bytes, which is real behaviour, not a missing observer.
        let cache = directory.join("npm-cache");
        fs::create_dir_all(&cache).unwrap();
        let package = std::env::var("PROBE_PACKAGE").unwrap_or_else(|_| "is-number@7.0.0".into());
        let mut command = pipeline
            .install_command(
                &directory,
                &[package.clone()],
                InstallOptions {
                    local_links: false,
                    observe: true,
                },
            )
            .unwrap();
        command.env("npm_config_cache", &cache);
        let result = run(
            command,
            &directory,
            Duration::from_secs(180),
            8,
            |_, _| {},
        );
        // Read before cleaning up, assert after: a failed assertion must not leak
        // the scratch directory under the temp path.
        let progress = fs::read_to_string(directory.join("download-progress.json")).unwrap_or_default();
        let installed = fs::read_dir(directory.join("node_modules"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        println!("package={package} result={result:?} installed={installed} progress={progress}");
        let _ = fs::remove_dir_all(&directory);
        assert!(result.is_ok(), "{result:?}");
        assert!(installed > 0);
        assert!(!progress.is_empty(), "no progress file was written");
        let document: serde_json::Value = serde_json::from_str(&progress).unwrap();
        assert!(
            document["downloadedBytes"].as_u64().unwrap_or(0) > 0,
            "the observer never saw a download: {progress}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn run_reports_the_log_tail_and_times_out() {
        let directory = scratch("run");
        fs::create_dir_all(&directory).unwrap();
        let mut failing = Command::new("cmd");
        failing.args(["/c", "echo first & echo second & exit /b 3"]);
        let failure = run(failing, &directory, Duration::from_secs(10), 8, |_, _| {}).unwrap_err();
        match failure {
            Failure::Exit { tail } => {
                assert!(tail.contains("first"), "{tail}");
                assert!(tail.contains("second"), "{tail}");
            }
            other => panic!("expected an exit failure, got {other:?}"),
        }
        let mut slow = Command::new("cmd");
        slow.args(["/c", "ping -n 4 127.0.0.1 > nul"]);
        let failure = run(slow, &directory, Duration::from_millis(300), 8, |_, _| {}).unwrap_err();
        assert!(matches!(failure, Failure::TimedOut), "{failure:?}");
        let _ = fs::remove_dir_all(&directory);
    }
}

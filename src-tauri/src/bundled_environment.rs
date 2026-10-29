use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

static TOOLS: OnceLock<BTreeMap<String, PathBuf>> = OnceLock::new();
static ROOT: OnceLock<PathBuf> = OnceLock::new();

#[derive(Deserialize)]
struct Manifest {
    schema_version: u32,
    platform: String,
    tools: Vec<Tool>,
    #[serde(default)]
    files: Vec<InventoryEntry>,
}

#[derive(Deserialize)]
struct Tool {
    id: String,
    path: String,
}

#[derive(Deserialize)]
struct InventoryEntry {
    path: String,
    bytes: u64,
    sha256: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Verifies the generated inventory so a truncated or tampered install fails
/// at startup with a clear message instead of a cryptic error at use time.
fn verify_inventory(root: &Path, manifest: &Manifest) -> Result<(), String> {
    for entry in &manifest.files {
        let relative = Path::new(&entry.path);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!("Unsafe bundled inventory path: {}", entry.path));
        }
        let path = root.join(relative);
        let bytes = fs::read(&path)
            .map_err(|error| format!("内置工具文件缺失 {}：{error}", entry.path))?;
        if bytes.len() as u64 != entry.bytes {
            return Err(format!("内置工具文件大小不符：{}", entry.path));
        }
        if sha256_hex(&bytes) != entry.sha256 {
            return Err(format!("内置工具文件校验失败：{}", entry.path));
        }
    }
    Ok(())
}

fn read_tools(root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let manifest_path = root.join("manifest.json");
    if !manifest_path.exists() {
        // Development builds intentionally have no offline tool bundle.
        return Ok(BTreeMap::new());
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid bundled environment manifest: {e}"))?;
    if manifest.schema_version != 1 || manifest.platform != "windows-x64" {
        return Err("Unsupported bundled environment manifest".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    verify_inventory(&root, &manifest)?;
    let mut tools = BTreeMap::new();
    for tool in manifest.tools {
        if !matches!(
            tool.id.as_str(),
            "adb" | "aapt2" | "java" | "apktool" | "jadx" | "bundletool" | "scrcpy" | "python"
        ) {
            return Err(format!("Unknown bundled tool: {}", tool.id));
        }
        let relative = Path::new(&tool.path);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("Unsafe bundled tool path: {}", tool.id));
        }
        let path = root
            .join(relative)
            .canonicalize()
            .map_err(|e| format!("Bundled tool {} is missing: {e}", tool.id))?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(format!("Invalid bundled tool path: {}", tool.id));
        }
        if tools.insert(tool.id, path).is_some() {
            return Err("Duplicate bundled tool id".into());
        }
    }
    Ok(tools)
}

pub(crate) fn initialize(resource_dir: &Path) -> Result<(), String> {
    let cache = crate::user_storage::directory("tools/bundled")?;
    let source = [
        resource_dir.join("resources/environment-tools"),
        resource_dir.join("environment-tools"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/environment-tools"),
    ].into_iter().enumerate().filter(|(index, _)| *index < 2 || cfg!(debug_assertions))
        .map(|(_, path)| path).find(|path| path.join("manifest.json").is_file());
    let root = if let Some(source) = source {
        let identity = fs::read(source.join("manifest.json")).map_err(|e| e.to_string())?;
        crate::user_storage::cache_bundle(&source, &cache, &identity)?
    } else {
        crate::user_storage::active_bundle(&cache).unwrap_or(cache)
    };
    let tools = read_tools(&root)?;
    if root.is_dir() {
        let _ = ROOT.set(root.canonicalize().unwrap_or(root));
    }
    TOOLS
        .set(tools)
        .map_err(|_| "Bundled environment already initialized".into())
}

/// Canonical directory holding the bundled offline toolchain, when present.
pub(crate) fn root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

pub(crate) fn tool_path(id: &str) -> Option<PathBuf> {
    TOOLS.get()?.get(id).filter(|path| path.is_file()).cloned()
}

pub(crate) fn build_tool_path(executable: &str) -> Option<PathBuf> {
    if !matches!(
        executable.to_ascii_lowercase().as_str(),
        "aapt.exe" | "aapt2.exe" | "zipalign.exe" | "apksigner.bat" | "d8.bat"
    ) {
        return None;
    }
    let path = tool_path("aapt2")?.parent()?.join(executable);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "android-tool-bundle-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir_all(path.join("adb")).unwrap();
            fs::write(path.join("adb/adb.exe"), b"test").unwrap();
            Self(path)
        }
        fn manifest(&self, tools: serde_json::Value) {
            fs::write(
                self.0.join("manifest.json"),
                serde_json::to_vec(
                    &serde_json::json!({"schema_version":1,"platform":"windows-x64","tools":tools}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_installed_paths_without_machine_specific_configuration() {
        let f = Fixture::new();
        f.manifest(serde_json::json!([{"id":"adb","path":"adb/adb.exe"}]));
        assert_eq!(
            read_tools(&f.0).unwrap()["adb"],
            f.0.join("adb/adb.exe").canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_unsafe_unknown_duplicate_and_missing_tools() {
        let f = Fixture::new();
        for tools in [
            serde_json::json!([{"id":"adb","path":"../adb.exe"}]),
            serde_json::json!([{"id":"adb","path":"C:/Windows/cmd.exe"}]),
            serde_json::json!([{"id":"custom","path":"adb/adb.exe"}]),
            serde_json::json!([{"id":"adb","path":"adb/missing.exe"}]),
            serde_json::json!([{"id":"adb","path":"adb/adb.exe"},{"id":"adb","path":"adb/adb.exe"}]),
        ] {
            f.manifest(tools);
            assert!(read_tools(&f.0).is_err());
        }
    }

    #[test]
    fn development_without_a_manifest_is_supported() {
        let f = Fixture::new();
        assert!(read_tools(&f.0).unwrap().is_empty());
    }

    #[test]
    fn inventory_accepts_matching_files_and_rejects_tampering() {
        let f = Fixture::new();
        let inventory = serde_json::json!([
            {"path":"adb/adb.exe","bytes":4,"sha256":sha256_hex(b"test")}
        ]);
        let manifest = |files: serde_json::Value| {
            fs::write(
                f.0.join("manifest.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema_version":1,
                    "platform":"windows-x64",
                    "tools":[{"id":"adb","path":"adb/adb.exe"}],
                    "files":files
                }))
                .unwrap(),
            )
            .unwrap();
        };

        manifest(inventory.clone());
        assert!(read_tools(&f.0).is_ok());

        // A modified file must be rejected before the app tries to use it.
        manifest(serde_json::json!([
            {"path":"adb/adb.exe","bytes":4,"sha256":"00".repeat(32)}
        ]));
        assert!(read_tools(&f.0).is_err());

        // So must a size mismatch and a missing entry.
        manifest(serde_json::json!([
            {"path":"adb/adb.exe","bytes":999,"sha256":sha256_hex(b"test")}
        ]));
        assert!(read_tools(&f.0).is_err());
        manifest(serde_json::json!([
            {"path":"adb/gone.exe","bytes":4,"sha256":sha256_hex(b"test")}
        ]));
        assert!(read_tools(&f.0).is_err());
        manifest(serde_json::json!([
            {"path":"../escape.exe","bytes":4,"sha256":sha256_hex(b"test")}
        ]));
        assert!(read_tools(&f.0).is_err());
    }
}

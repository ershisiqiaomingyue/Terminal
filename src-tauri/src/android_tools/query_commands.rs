//! Blocking device and filesystem queries run away from the window thread.
use super::*;

async fn run_query<T: Send + 'static>(
    query: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(query)
        .await
        .map_err(|error| format!("后台查询任务失败：{error}"))?
}

// Keep the synchronous implementations available to existing background jobs.
// Only the desktop IPC entry points dispatch to the blocking worker pool.
macro_rules! query_commands {
    ($($name:ident($($argument:ident: $kind:ty),*) -> $output:ty;)+) => {
        $(#[tauri::command]
        pub async fn $name($($argument: $kind),*) -> Result<$output, String> {
            run_query(move || super::$name($($argument),*)).await
        })+
    };
}

query_commands! {
    list_adb_devices() -> Vec<AdbDevice>;
    list_adb_devices_passive() -> Vec<AdbDevice>;
    list_adb_packages(serial: String) -> Vec<String>;
    list_adb_processes(serial: String) -> HashMap<String, String>;
    get_adb_device_info(serial: String) -> AdbDeviceInfo;
    get_adb_device_status(serial: String) -> AdbDeviceStatus;
    get_adb_storage_info(serial: String, path: String) -> AdbStorageInfo;
    get_adb_capture_info(serial: String) -> AdbCaptureInfo;
    list_adb_applications(serial: String) -> Vec<AdbApplication>;
    list_adb_process_details(serial: String) -> Vec<AdbProcess>;
    get_adb_package_details(serial: String, package_name: String) -> AdbPackageDetails;
    list_adb_remote_files(serial: String, path: String) -> Vec<AdbRemoteFile>;
    list_adb_backups(app: AppHandle, serial: String) -> Vec<AdbBackupEntry>;
    get_adb_backup_directory(app: AppHandle, serial: String) -> String;
    get_adb_privilege_status(serial: String) -> AdbPrivilegeStatus;
    read_adb_remote_text(serial: String, path: String) -> AdbRemoteText;
    symbolize_native_tombstone(request: NativeSymbolizeRequest) -> NativeSymbolizeResult;
}

#[cfg(test)]
mod tests {
    use super::run_query;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_query_leaves_runtime_free_to_release_its_worker() {
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let query = tokio::spawn(run_query(move || {
            let _ = started.send(());
            wait.recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|error| error.to_string())?;
            Ok(42)
        }));
        ready.await.unwrap();
        release.send(()).unwrap();
        assert_eq!(query.await.unwrap().unwrap(), 42);
    }

    #[tokio::test]
    async fn query_errors_reach_the_calling_view() {
        assert_eq!(
            run_query::<()>(|| Err("设备已断开".into()))
                .await
                .unwrap_err(),
            "设备已断开"
        );
    }
}

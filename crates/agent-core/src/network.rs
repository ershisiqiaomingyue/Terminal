use reqwest::{ClientBuilder, Proxy};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use url::Url;

static HTTP_PROXY_SETTINGS: OnceLock<RwLock<HttpProxySettings>> = OnceLock::new();

fn proxy_store() -> &'static RwLock<HttpProxySettings> {
    HTTP_PROXY_SETTINGS.get_or_init(|| RwLock::new(HttpProxySettings::default()))
}

#[derive(Default, Clone)]
pub struct HttpProxySettings {
    proxy_url: Option<String>,
    enabled: bool,
    child_processes: bool,
}

impl HttpProxySettings {
    pub fn new(
        proxy_url: Option<String>,
        enabled: bool,
        child_processes: bool,
    ) -> Result<Self, String> {
        let proxy_url = proxy_url
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(value) = proxy_url.as_deref() {
            let parsed = Url::parse(value).map_err(|error| format!("代理地址格式无效：{error}"))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err("代理地址仅支持包含主机的 HTTP 或 HTTPS URL。".to_string());
            }
            Proxy::all(value).map_err(|error| format!("代理地址不可用：{error}"))?;
        } else if enabled {
            return Err("启用代理前请填写代理地址。".to_string());
        }
        Ok(Self {
            proxy_url,
            enabled,
            child_processes,
        })
    }

    fn active_url(&self) -> Option<&str> {
        self.proxy_url.as_deref().filter(|_| self.enabled)
    }

    fn client_builder(&self) -> Result<ClientBuilder, String> {
        let builder = ClientBuilder::new().no_proxy();
        if let Some(proxy_url) = self.active_url() {
            let proxy =
                Proxy::all(proxy_url).map_err(|error| format!("代理地址不可用：{error}"))?;
            Ok(builder.proxy(proxy))
        } else {
            Ok(builder)
        }
    }

    fn child_environment(&self) -> Vec<(String, String)> {
        let Some(proxy_url) = self.active_url().filter(|_| self.child_processes) else {
            return Vec::new();
        };
        [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ]
        .into_iter()
        .map(|name| (name.to_string(), proxy_url.to_string()))
        .collect()
    }
}

/// Environment overrides that every child process should inherit so the tools
/// shipped with the application are reachable from shells the agent or the
/// integrated terminal starts, not just from the desktop process itself.
static CHILD_ENVIRONMENT: OnceLock<RwLock<ChildEnvironment>> = OnceLock::new();

#[derive(Default, Clone)]
pub struct ChildEnvironment {
    path_entries: Vec<PathBuf>,
    variables: BTreeMap<String, String>,
}

fn child_environment_store() -> &'static RwLock<ChildEnvironment> {
    CHILD_ENVIRONMENT.get_or_init(|| RwLock::new(ChildEnvironment::default()))
}

pub fn set_child_environment(
    path_entries: Vec<PathBuf>,
    variables: BTreeMap<String, String>,
) -> Result<(), String> {
    let store = child_environment_store();
    let mut current = store
        .write()
        .map_err(|_| "子进程环境配置状态不可用。".to_string())?;
    current.path_entries = path_entries;
    current.variables = variables;
    Ok(())
}

/// Prepends the configured tool directories to `PATH` and applies the extra
/// variables, leaving the inherited environment intact behind them.
pub fn apply_child_environment(command: &mut tokio::process::Command) {
    let Ok(store) = child_environment_store().read() else {
        return;
    };
    if !store.path_entries.is_empty() {
        let mut entries = store.path_entries.clone();
        if let Some(existing) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&existing));
        }
        if let Ok(path) = std::env::join_paths(entries) {
            command.env("PATH", path);
        }
    }
    for (name, value) in &store.variables {
        command.env(name, value);
    }
    for (name, value) in child_proxy_environment() {
        command.env(name, value);
    }
}

/// Same as [`apply_child_environment`] for the blocking `std::process::Command`
/// used when launching the interactive terminal shells.
pub fn apply_child_environment_std(command: &mut std::process::Command) {
    let Ok(store) = child_environment_store().read() else {
        return;
    };
    if !store.path_entries.is_empty() {
        let mut entries = store.path_entries.clone();
        if let Some(existing) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&existing));
        }
        if let Ok(path) = std::env::join_paths(entries) {
            command.env("PATH", path);
        }
    }
    for (name, value) in &store.variables {
        command.env(name, value);
    }
    for (name, value) in child_proxy_environment() {
        command.env(name, value);
    }
}

/// Directories that should be searched ahead of `PATH` when resolving a bare
/// executable name, so the agent can run `adb` or `java` on a clean machine.
pub fn child_environment_path_entries() -> Vec<PathBuf> {
    child_environment_store()
        .read()
        .map(|store| store.path_entries.clone())
        .unwrap_or_default()
}

pub fn set_http_proxy_settings(settings: HttpProxySettings) -> Result<(), String> {
    *proxy_store()
        .write()
        .map_err(|_| "代理配置状态不可用。".to_string())? = settings;
    Ok(())
}

pub fn http_client_builder() -> Result<ClientBuilder, String> {
    proxy_store()
        .read()
        .map_err(|_| "代理配置状态不可用。".to_string())?
        .client_builder()
}

pub fn child_proxy_environment() -> Vec<(String, String)> {
    proxy_store()
        .read()
        .map(|settings| settings.child_environment())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn proxy_policy_validates_and_keeps_disabled_address() {
        assert!(HttpProxySettings::new(None, true, false).is_err());
        for url in ["not-a-proxy", "socks5://127.0.0.1:7890", "file:///proxy"] {
            assert!(HttpProxySettings::new(Some(url.into()), true, false).is_err());
        }
        let settings =
            HttpProxySettings::new(Some(" http://127.0.0.1:7890 ".into()), false, true).unwrap();
        assert_eq!(settings.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(settings.active_url(), None);
        assert!(settings.child_environment().is_empty());
        assert!(HttpProxySettings::new(None, false, false).is_ok());
    }

    #[tokio::test]
    async fn proxy_policy_routes_http_and_controls_spawned_commands() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "proxy did not receive a request"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("proxy accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 1024];
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
            }
            assert!(String::from_utf8_lossy(&received)
                .starts_with("GET http://proxy-target.invalid/test "));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nproxied",
                )
                .unwrap();
        });
        let application_only =
            HttpProxySettings::new(Some(proxy_url.clone()), true, false).unwrap();
        assert!(application_only.child_environment().is_empty());
        let response = application_only
            .client_builder()
            .unwrap()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get("http://proxy-target.invalid/test")
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "proxied");
        server.join().unwrap();

        let disabled = HttpProxySettings::new(Some(proxy_url.clone()), false, true).unwrap();
        // The proxy listener has closed. A local origin must still be reachable with the master switch off.
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_url = format!("http://{}", origin.local_addr().unwrap());
        origin.set_nonblocking(true).unwrap();
        let origin_server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok((mut stream, _)) = origin.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut buffer = [0; 2048];
                    assert!(stream.read(&mut buffer).unwrap() > 0);
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ndirect").unwrap();
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let response = disabled
            .client_builder()
            .unwrap()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(origin_url)
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "direct");
        origin_server.join().unwrap();

        for (enabled, child_processes, expected) in [
            (true, false, false),
            (true, true, true),
            (false, true, false),
        ] {
            let settings =
                HttpProxySettings::new(Some(proxy_url.clone()), enabled, child_processes).unwrap();
            let environment = settings.child_environment();
            #[cfg(windows)]
            let mut command = {
                let mut command = std::process::Command::new("cmd.exe");
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
                command.args(["/d", "/c", "echo %HTTP_PROXY%"]);
                command
            };
            #[cfg(not(windows))]
            let mut command = {
                let mut command = std::process::Command::new("sh");
                command.args(["-c", "printf '%s' \"$HTTP_PROXY\""]);
                command
            };
            for name in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                command.env_remove(name);
            }
            command.envs(environment.iter().map(|(name, value)| (name, value)));
            let output = command.output().unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim() == proxy_url,
                expected
            );
            assert_eq!(environment.len(), if expected { 6 } else { 0 });
            if expected {
                assert!(environment
                    .iter()
                    .any(|(name, value)| name == "HTTPS_PROXY" && value == &proxy_url));
                assert!(environment
                    .iter()
                    .any(|(name, value)| name == "https_proxy" && value == &proxy_url));
            }
        }
    }
}

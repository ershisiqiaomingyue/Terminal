//! Loopback-only SSH server with test-owned keys and no system configuration writes.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct SshFixture {
    pub distribution: String,
    pub remote_root: String,
    pub local_root: PathBuf,
    pub config: Value,
    server: Option<Child>,
    externally_managed: bool,
}

struct ProxyState {
    stopped: bool,
    rejecting: bool,
    revision: u64,
    streams: Vec<TcpStream>,
}

/// Drops test-owned TCP transports without touching firewall or SSH settings.
pub struct SshFaultProxy {
    pub config: Value,
    state: Arc<Mutex<ProxyState>>,
    worker: Option<JoinHandle<()>>,
}

impl SshFaultProxy {
    pub fn start(fixture: &SshFixture) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = format!("127.0.0.1:{}", fixture.config["port"].as_u64().unwrap())
            .parse()
            .unwrap();
        let known_hosts = fixture.local_root.join(format!("proxy-{port}-known_hosts"));
        let original =
            std::fs::read_to_string(fixture.config["knownHostsPath"].as_str().unwrap()).unwrap();
        let (_, key) = original.split_once(' ').unwrap();
        std::fs::write(&known_hosts, format!("[127.0.0.1]:{port} {key}")).unwrap();
        let mut config = fixture.config.clone();
        config["port"] = json!(port);
        config["knownHostsPath"] = json!(known_hosts);
        config["id"] = json!(format!("fault-proxy-{port}"));
        let state = Arc::new(Mutex::new(ProxyState {
            stopped: false,
            rejecting: false,
            revision: 0,
            streams: Vec::new(),
        }));
        let worker_state = state.clone();
        let worker = std::thread::spawn(move || {
            let mut pumps = Vec::new();
            loop {
                if worker_state.lock().unwrap().stopped {
                    break;
                }
                let client = match listener.accept() {
                    Ok((client, _)) => client,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("SSH fault proxy accept failed: {error}"),
                };
                let mut state = worker_state.lock().unwrap();
                if state.stopped || state.rejecting {
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                }
                client.set_nonblocking(false).unwrap();
                let server = TcpStream::connect_timeout(&target, Duration::from_secs(1)).unwrap();
                for stream in [&client, &server] {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    stream.set_nodelay(true).unwrap();
                }
                state.streams.push(client.try_clone().unwrap());
                state.streams.push(server.try_clone().unwrap());
                let revision = state.revision;
                let client_read = client.try_clone().unwrap();
                let server_write = server.try_clone().unwrap();
                let forward_state = worker_state.clone();
                pumps.push(std::thread::spawn(move || {
                    proxy_forward(client_read, server_write, forward_state, revision);
                }));
                let forward_state = worker_state.clone();
                pumps.push(std::thread::spawn(move || {
                    proxy_forward(server, client, forward_state, revision);
                }));
            }
            for pump in pumps {
                pump.join().unwrap();
            }
        });
        Self {
            config,
            state,
            worker: Some(worker),
        }
    }

    pub fn drop_connections(&self, reject_new: bool) {
        let mut state = self.state.lock().unwrap();
        state.rejecting = reject_new;
        state.revision += 1;
        for stream in state.streams.drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub fn restore(&self) {
        self.state.lock().unwrap().rejecting = false;
    }
}

fn proxy_forward(
    mut source: TcpStream,
    mut target: TcpStream,
    state: Arc<Mutex<ProxyState>>,
    revision: u64,
) {
    let stopped = || {
        let state = state.lock().unwrap();
        state.stopped || state.revision != revision
    };
    let mut buffer = [0u8; 8192];
    'forward: while !stopped() {
        let length = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => length,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue
            }
            Err(_) => break,
        };
        let mut offset = 0;
        while offset < length {
            if stopped() {
                break 'forward;
            }
            match target.write(&buffer[offset..length]) {
                Ok(0) => break 'forward,
                Ok(written) => offset += written,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(_) => break 'forward,
            }
        }
    }
    let _ = source.shutdown(Shutdown::Both);
    let _ = target.shutdown(Shutdown::Both);
}

impl Drop for SshFaultProxy {
    fn drop(&mut self) {
        self.state.lock().unwrap().stopped = true;
        self.drop_connections(true);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn wsl_command(distribution: &str, script: &str, args: &[&str], input: Option<&[u8]>) -> Output {
    let mut command = Command::new("wsl.exe");
    command
        .creation_flags(0x08000000)
        .args([
            "-d",
            distribution,
            "--exec",
            "sh",
            "-c",
            script,
            "dsh-ssh-fixture",
        ])
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("start fixture WSL command");
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    child.wait_with_output().expect("drain fixture WSL command")
}

impl SshFixture {
    pub fn start(distribution: &str) -> Self {
        Self::start_with_host_key(distribution, "rsa")
    }

    pub fn start_with_host_key(distribution: &str, key_type: &str) -> Self {
        assert!(matches!(key_type, "rsa" | "ed25519"));
        // A host-owned fixture also works when a restricted test process can
        // connect to loopback but cannot launch WSL itself.
        if let Ok(path) = std::env::var("ANDROID_DSH_TEST_SSH_FIXTURE") {
            let descriptor: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!(descriptor["config"]["host"], "127.0.0.1");
            let remote_root = descriptor["remoteRoot"].as_str().unwrap().to_string();
            assert!(remote_root
                .strip_prefix("/tmp/android-tool-ssh.")
                .is_some_and(|suffix| suffix.len() == 12
                    && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())));
            return Self {
                distribution: distribution.into(),
                remote_root,
                local_root: descriptor["localRoot"].as_str().unwrap().into(),
                config: descriptor["config"].clone(),
                server: None,
                externally_managed: true,
            };
        }
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let local_root = std::env::temp_dir().join(format!(
            "android-tool-ssh-test-{}-{suffix}",
            std::process::id()
        ));
        assert!(!local_root.exists());
        std::fs::create_dir(&local_root).unwrap();
        let prepared = wsl_command(
            distribution,
            "umask 077; mktemp -d /tmp/android-tool-ssh.XXXXXXXXXXXX; id -un",
            &[],
            None,
        );
        assert!(
            prepared.status.success(),
            "SSH fixture preparation failed: {} {}",
            String::from_utf8_lossy(&prepared.stdout),
            String::from_utf8_lossy(&prepared.stderr)
        );
        let prepared = String::from_utf8(prepared.stdout).unwrap();
        let mut lines = prepared.lines();
        let remote_root = lines.next().unwrap().to_string();
        let username = lines.next().unwrap().to_string();
        assert!(remote_root
            .strip_prefix("/tmp/android-tool-ssh.")
            .is_some_and(|suffix| suffix.len() == 12
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())));
        assert!(!username.is_empty() && !username.contains(char::is_whitespace));
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client_key = local_root.join("client.pem");
        let known_hosts = local_root.join("known_hosts");
        let mut fixture = Self {
            distribution: distribution.into(),
            remote_root,
            local_root,
            server: None,
            externally_managed: false,
            config: json!({"id":format!("isolated-ssh-{suffix}"),"name":"Isolated SSH fixture",
                "host":"127.0.0.1","port":port,"username":username,"privateKeyPath":client_key,
                "knownHostsPath":known_hosts,"strictHostKey":true,"reuseConnection":true,
                "connectTimeoutSecs":5,"operationTimeoutSecs":15,"keepaliveIntervalSecs":5}),
        };
        let generated = Command::new("ssh-keygen.exe")
            .creation_flags(0x08000000)
            .args(["-q", "-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "", "-f"])
            .arg(&client_key)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "test client key generation failed: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let key = std::fs::read(client_key.with_extension("pem.pub")).unwrap();
        let prepared = wsl_command(distribution,
            "umask 077; cat > \"$1/authorized_keys\"; ssh-keygen -q -t \"$2\" -b 2048 -N '' -f \"$1/hostkey\"; cat \"$1/hostkey.pub\"",
            &[&fixture.remote_root, key_type], Some(&key));
        assert!(
            prepared.status.success(),
            "test server key preparation failed"
        );
        let host_key = String::from_utf8(prepared.stdout).unwrap();
        let mut fields = host_key.split_whitespace();
        let key_type = fields.next().unwrap();
        let public_key = fields.next().unwrap();
        std::fs::write(
            &known_hosts,
            format!("[127.0.0.1]:{port} {key_type} {public_key}\n"),
        )
        .unwrap();
        fixture.config["defaultRoot"] = json!(fixture.remote_root);
        let root = fixture.remote_root.clone();
        let server = Command::new("wsl.exe")
            .creation_flags(0x08000000)
            .args([
                "-d",
                distribution,
                "--exec",
                "env",
                &format!("ANDROID_DSH_SSH_TEST={root}"),
                "/usr/sbin/sshd",
                "-D",
                "-e",
                "-f",
                "/dev/null",
                "-h",
                &format!("{root}/hostkey"),
                "-p",
                &port.to_string(),
                "-o",
                "ListenAddress=127.0.0.1",
                "-o",
                &format!("PidFile={root}/sshd.pid"),
                "-o",
                &format!("AuthorizedKeysFile={root}/authorized_keys"),
                "-o",
                "StrictModes=no",
                "-o",
                "PasswordAuthentication=no",
                "-o",
                "KbdInteractiveAuthentication=no",
                "-o",
                "UsePAM=no",
                "-o",
                "PubkeyAuthentication=yes",
                "-o",
                &format!("AllowUsers={username}"),
                "-o",
                "PrintMotd=no",
                "-o",
                "LogLevel=ERROR",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        fixture.server = Some(server);
        let started = Instant::now();
        loop {
            if let Some(status) = fixture.server.as_mut().unwrap().try_wait().unwrap() {
                let server = fixture.server.take().unwrap().wait_with_output().unwrap();
                panic!(
                    "isolated sshd exited {status}: {}",
                    String::from_utf8_lossy(&server.stderr)
                );
            }
            if TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "isolated sshd did not listen"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        fixture
    }

    pub fn probe_and_clean_test_process(&self, pid: u32, marker: &str) -> String {
        let probe = wsl_command(&self.distribution,
            "if test -r /proc/$1/environ && tr '\\0' '\\n' < /proc/$1/environ | grep -Fxq \"ANDROID_DSH_SSH_CHILD=$2\"; then ps -o stat= -p \"$1\"; kill -KILL \"$1\"; fi",
            &[&pid.to_string(), marker], None);
        assert!(probe.status.success());
        String::from_utf8(probe.stdout).unwrap()
    }
}

impl Drop for SshFixture {
    fn drop(&mut self) {
        if self.externally_managed {
            return;
        }
        // The server's unique inherited marker is checked before signalling its PID.
        let cleanup = wsl_command(
            &self.distribution,
            r#"
root=$1
case "$root" in /tmp/android-tool-ssh.*) ;; *) exit 125;; esac
test -d "$root" && test ! -L "$root" || exit 125
if test -r "$root/sshd.pid"; then
    read -r pid < "$root/sshd.pid"
    case "$pid" in ''|*[!0-9]*) exit 125;; esac
    if test -r "/proc/$pid/environ" && tr '\0' '\n' < "/proc/$pid/environ" | grep -Fxq "ANDROID_DSH_SSH_TEST=$root"; then
        kill -TERM "$pid"
    fi
fi
rm -f -- "$root/sshd.pid" "$root/hostkey" "$root/hostkey.pub" "$root/authorized_keys"
rmdir -- "$root"
"#,
            &[&self.remote_root],
            None,
        );
        if !cleanup.status.success() {
            eprintln!("SSH fixture cleanup incomplete: {}", self.remote_root);
        }
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            if let Ok(output) = server.wait_with_output() {
                if std::thread::panicking() {
                    eprintln!(
                        "Isolated sshd diagnostic: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
        }
        if self
            .local_root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("android-tool-ssh-test-")
        {
            let _ = std::fs::remove_dir_all(&self.local_root);
        }
    }
}

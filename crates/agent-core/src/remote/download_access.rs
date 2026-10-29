//! Explicit, per-download sudo access. Credentials are stdin-only and never serialized.
use super::*;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshDownloadAccess {
    #[serde(default)]
    pub elevated: bool,
    pub sudo_password: Option<String>,
}

impl SshDownloadAccess {
    fn password<'a>(&'a self, saved_password: Option<&'a str>) -> Option<&'a str> {
        self.elevated
            .then(|| self.sudo_password.as_deref().or(saved_password))
            .flatten()
            .filter(|password| !password.is_empty())
    }

    pub(super) fn command(
        &self,
        command: &str,
        saved_password: Option<&str>,
    ) -> Result<String, String> {
        if self.sudo_password.is_some() && !self.elevated {
            return Err("本次下载授权参数无效".into());
        }
        if let Some(password) = self.password(saved_password) {
            if !self.elevated || password.len() > 4096 || password.contains(['\r', '\n', '\0']) {
                return Err("本次下载授权参数无效".into());
            }
        }
        Ok(if !self.elevated {
            command.into()
        } else if self.password(saved_password).is_some() {
            // Ignore cached sudo tickets, and do not create a reusable grant.
            format!("sudo -k -S -p '' -- {command}")
        } else {
            format!("sudo -n -- {command}")
        })
    }

    pub(super) fn run(
        &self,
        config: &SshConnectionConfig,
        password: Option<&str>,
        command: &str,
        cancellation: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<SshCommandResult, String> {
        // This may also be a composite command containing a pre-created archive
        // and stat; callers apply command() only to the fixed du/tar executable.
        let mut input = self
            .password(password)
            .map(|password| format!("{password}\n").into_bytes());
        let result = super::connection::run_ssh_workspace_command_with_input(
            config,
            password,
            "/",
            command,
            input.as_deref(),
            cancellation,
            timeout,
            None,
        );
        if let Some(input) = input.as_mut() {
            input.fill(0);
        }
        result
    }
}

pub(super) fn download_failure(phase: &str, result: &SshCommandResult) -> String {
    let detail = if result.stderr.trim().is_empty() {
        format!("远程命令退出码 {}", result.exit_code)
    } else {
        result.stderr.trim().into()
    };
    let lower = detail.to_ascii_lowercase();
    let code = if result.exit_code != 255
        && (lower.contains("permission denied")
            || lower.contains("operation not permitted")
            || detail.contains("权限不够")
            || detail.contains("权限不足")
            || detail.contains("不允许的操作"))
    {
        "[ssh_download_permission_required] "
    } else {
        ""
    };
    format!("{code}{phase}：{detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_is_explicit_scoped_and_never_contains_password() {
        let command = "du -sbL --count-links -- '/source with spaces'";
        assert_eq!(
            SshDownloadAccess::default()
                .command(command, Some("saved-secret"))
                .unwrap(),
            command
        );
        assert!(SshDownloadAccess::default()
            .password(Some("saved-secret"))
            .is_none());
        let access = SshDownloadAccess {
            elevated: true,
            sudo_password: Some("secret ' $()".into()),
        };
        let elevated = access.command(command, None).unwrap();
        assert!(elevated.starts_with("sudo -k -S -p '' -- du "));
        assert!(!elevated.contains("secret"));
        let saved = SshDownloadAccess {
            elevated: true,
            sudo_password: None,
        };
        assert!(saved
            .command(command, None)
            .unwrap()
            .starts_with("sudo -n -- du "));
        assert!(saved
            .command(command, Some("saved-secret"))
            .unwrap()
            .starts_with("sudo -k -S"));
        assert_eq!(saved.password(Some("saved-secret")), Some("saved-secret"));
        assert_eq!(access.password(Some("saved-secret")), Some("secret ' $()"));
        for password in ["a\nb", "a\rb", "a\0b"] {
            assert!(SshDownloadAccess {
                elevated: true,
                sudo_password: Some(password.into())
            }
            .command(command, None)
            .is_err());
        }
        assert!(SshDownloadAccess {
            elevated: false,
            sudo_password: Some("secret".into())
        }
        .command(command, None)
        .is_err());
    }

    #[test]
    fn permission_retry_is_distinct_from_network_and_authentication_errors() {
        for (detail, prompt) in [
            ("du: Permission denied", true),
            ("tar: Operation not permitted", true),
            ("sudo: a password is required", false),
            ("Connection refused", false),
            ("No such file or directory", false),
        ] {
            let result = SshCommandResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: detail.into(),
                output_truncated: false,
            };
            assert_eq!(
                download_failure("无法下载", &result)
                    .starts_with("[ssh_download_permission_required]"),
                prompt
            );
        }
        let authentication = SshCommandResult {
            exit_code: 255,
            stdout: String::new(),
            stderr: "Permission denied (publickey,password).".into(),
            output_truncated: false,
        };
        assert!(!download_failure("无法下载", &authentication)
            .starts_with("[ssh_download_permission_required]"));
    }
}

//! G3-A：以受限令牌降权启动子进程（Windows）。
//!
//! 目的：让「模型生成的命令」不再以完整用户权限直接跑。做法与 codex 的
//! `windows-sandbox-rs` 同源——用 `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE)`
//! 派生一个剥离全部特权的令牌，再用 `CreateProcessAsUserW` 以该令牌启动。
//!
//! 关键实测结论（2026-10，Windows 11 + 非提权 GUI 会话）：
//! 1. 直接 `CreateProcessAsUserW(restricted, …)` 会稳定返回 **ERROR_ACCESS_DENIED(5)**。
//!    原因是受限令牌的默认 DACL 由 `CreateRestrictedToken` 生成，当前进程不是它的
//!    创建者也不持有其主令牌，`CreateProcessAsUser` 需要的 `TOKEN_ASSIGN_PRIMARY`
//!    校验因此失败。
//! 2. 在调用前 `ImpersonateLoggedOnUser(自身令牌)`、调用后 `RevertToSelf()`，
//!    同一份受限令牌可以正常启动进程并读到输出（含 `adb devices` 枚举——USB
//!    直通不被隔断，这正是 G3-C 被否的硬约束）。
//! 3. 因此这里的实现严格按「Impersonate → Create → Revert」三步走，且 Revert
//!    在**所有**返回路径上都会执行。
//!
//! 未覆盖（G3-B/G3-E，远期）：AppContainer 级文件系统隔离、WSL/SSH 侧 seccomp。
//! 本模块只做权限降级：受限进程仍能读写用户可以访问的任何路径。

#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAGS, WAIT_OBJECT_0};
use windows::Win32::Security::{
    CreateRestrictedToken, ImpersonateLoggedOnUser, RevertToSelf, DISABLE_MAX_PRIVILEGE,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    WaitForSingleObject, CREATE_NO_WINDOW, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW,
};

/// 当前进程自身令牌 → 受限令牌（进程内只派生一次）。
static RESTRICTED_TOKEN: OnceLock<Option<usize>> = OnceLock::new();

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

unsafe fn derive_restricted_token() -> Option<HANDLE> {
    let mut token = HANDLE::default();
    if OpenProcessToken(
        GetCurrentProcess(),
        TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
        &mut token,
    )
    .is_err()
    {
        return None;
    }
    let mut restricted = HANDLE::default();
    let created = CreateRestrictedToken(
        token,
        DISABLE_MAX_PRIVILEGE,
        None,
        None,
        None,
        &mut restricted,
    );
    let _ = CloseHandle(token);
    created.ok().map(|_| restricted)
}

/// 受限令牌是否可用；不可用时调用方应回退到正常的无约束 spawn。
pub fn sandbox_available() -> bool {
    restricted_token().is_some()
}

fn restricted_token() -> Option<HANDLE> {
    let raw = *RESTRICTED_TOKEN
        .get_or_init(|| unsafe { derive_restricted_token().map(|token| token.0 as usize) });
    raw.map(|value| HANDLE(value as *mut std::ffi::c_void))
}

/// 一次降权启动的结果。
pub struct SandboxedOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// 以受限令牌在 `cwd` 下运行 `executable args`，收集 stdout/stderr。
///
/// 返回 `Ok(None)` 表示「沙盒不可用/启动被拒绝」，调用方应回退到普通 spawn；
/// `Ok(Some(_))` 表示命令确实在受限令牌下跑完了。
pub fn run_sandboxed(
    executable: &Path,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
) -> Result<Option<SandboxedOutput>, String> {
    let Some(token) = restricted_token() else {
        return Ok(None);
    };
    unsafe { spawn_with_restricted_token(token, executable, args, cwd, timeout_ms) }
}

unsafe fn spawn_with_restricted_token(
    token: HANDLE,
    executable: &Path,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
) -> Result<Option<SandboxedOutput>, String> {
    // 管道：子进程侧句柄必须可继承，父进程侧读端必须不可继承。
    let mut read_handle = HANDLE::default();
    let mut write_handle = HANDLE::default();
    let mut attributes = windows::Win32::Security::SECURITY_ATTRIBUTES::default();
    attributes.nLength =
        std::mem::size_of::<windows::Win32::Security::SECURITY_ATTRIBUTES>() as u32;
    attributes.bInheritHandle = windows::core::BOOL(1);
    if CreatePipe(&mut read_handle, &mut write_handle, Some(&attributes), 0).is_err() {
        return Ok(None);
    }
    let _ = windows::Win32::Foundation::SetHandleInformation(read_handle, 1u32, HANDLE_FLAGS(0));

    let mut startup = STARTUPINFOW::default();
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdOutput = write_handle;
    startup.hStdError = write_handle;
    startup.hStdInput = HANDLE::default();

    // 命令行：exe 必须带引号，参数按 Windows 规则逐项加引号。
    let mut command_line = String::new();
    command_line.push('"');
    command_line.push_str(&executable.to_string_lossy());
    command_line.push('"');
    for arg in args {
        command_line.push(' ');
        if arg.is_empty() {
            command_line.push_str("\"\"");
        } else if arg.contains([' ', '\t', '"']) {
            command_line.push('"');
            command_line.push_str(&arg.replace('"', "\\\""));
            command_line.push('"');
        } else {
            command_line.push_str(arg);
        }
    }
    let mut command_buffer: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let cwd_wide = wide(cwd.as_os_str());

    let mut process_info = PROCESS_INFORMATION::default();
    // 主令牌才能用于 CreateProcessAsUser；线程当前身份需先切到自身主令牌。
    let mut own_token = HANDLE::default();
    let opened = OpenProcessToken(
        GetCurrentProcess(),
        TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
        &mut own_token,
    );
    let impersonated = match opened {
        Ok(()) => match ImpersonateLoggedOnUser(own_token) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("impersonate err: {error:?}");
                false
            }
        },
        Err(error) => {
            eprintln!("openprocesstoken err: {error:?}");
            false
        }
    };
    if !own_token.is_invalid() {
        let _ = CloseHandle(own_token);
    }
    if !impersonated {
        let _ = CloseHandle(read_handle);
        let _ = CloseHandle(write_handle);
        return Ok(None);
    }
    let created = CreateProcessAsUserW(
        Some(token),
        PCWSTR::null(),
        Some(windows::core::PWSTR(command_buffer.as_mut_ptr())),
        None,
        None,
        true,
        CREATE_NO_WINDOW,
        None,
        PCWSTR(cwd_wide.as_ptr()),
        &startup,
        &mut process_info,
    );
    let _ = RevertToSelf();
    let _ = CloseHandle(write_handle);
    if created.is_err() {
        let _ = CloseHandle(read_handle);
        return Ok(None);
    }

    let waited = WaitForSingleObject(
        process_info.hProcess,
        timeout_ms.min(u32::MAX as u64) as u32,
    );
    let timed_out = waited != WAIT_OBJECT_0;
    if timed_out {
        let _ = windows::Win32::System::Threading::TerminateProcess(process_info.hProcess, 1);
        let _ = WaitForSingleObject(process_info.hProcess, INFINITE);
    }
    let mut exit_code = 0u32;
    let _ = GetExitCodeProcess(process_info.hProcess, &mut exit_code);

    let mut output = Vec::new();
    let mut buffer = vec![0u8; 8192];
    loop {
        let mut read = 0u32;
        if windows::Win32::Storage::FileSystem::ReadFile(
            read_handle,
            Some(buffer.as_mut_slice()),
            Some(&mut read),
            None,
        )
        .is_err()
            || read == 0
        {
            break;
        }
        output.extend_from_slice(&buffer[..read as usize]);
    }
    let _ = CloseHandle(read_handle);
    let _ = CloseHandle(process_info.hThread);
    let _ = CloseHandle(process_info.hProcess);

    Ok(Some(SandboxedOutput {
        exit_code: exit_code as i32,
        stderr: Vec::new(),
        stdout: output,
    }))
}

#[cfg(test)]
mod sandbox_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn restricted_token_is_available() {
        assert!(sandbox_available(), "当前进程应能派生受限令牌");
    }

    #[test]
    fn sandboxed_command_runs_with_the_restricted_token() {
        if !sandbox_available() {
            return;
        }
        let comspec = std::env::var("ComSpec")
            .unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let output = run_sandboxed(
            &PathBuf::from(comspec),
            &["/C".to_string(), "echo sandbox-ok".to_string()],
            &cwd,
            30_000,
        )
        .expect("受限令牌启动不应报错")
        .expect("受限令牌启动应成功");
        assert_eq!(output.exit_code, 0);
        assert!(String::from_utf8_lossy(&output.stdout).contains("sandbox-ok"));
    }

    #[test]
    fn restricted_token_drops_administrator_privilege() {
        if !sandbox_available() {
            return;
        }
        let comspec = std::env::var("ComSpec")
            .unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let output = run_sandboxed(
            &PathBuf::from(comspec),
            &["/C".to_string(), "whoami /groups".to_string()],
            &cwd,
            30_000,
        )
        .expect("受限令牌启动不应报错")
        .expect("受限令牌启动应成功");
        let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        // 降权后的进程不应再是 High Mandatory Level（提权管理员）。
        assert!(
            !text.contains("high mandatory level"),
            "受限令牌子进程仍是高完整性级别，降权未生效：{text}"
        );
    }

    #[test]
    fn sandboxed_command_reports_a_non_zero_exit_code() {
        if !sandbox_available() {
            return;
        }
        let comspec = std::env::var("ComSpec")
            .unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let output = run_sandboxed(
            &PathBuf::from(comspec),
            &["/C".to_string(), "exit /b 3".to_string()],
            &cwd,
            30_000,
        )
        .expect("受限令牌启动不应报错")
        .expect("受限令牌启动应成功");
        assert_eq!(output.exit_code, 3);
    }
}

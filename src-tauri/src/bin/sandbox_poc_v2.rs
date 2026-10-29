//! G3 spike v2：受限令牌下的 **stdio 管道桥** 验证。
//!
//! v1（同文件前一段历史）证明了 CreateRestrictedToken + CreateProcessAsUserW
//! 可行、adb 直通不受降权影响。v2 补上产品化的下一个不确定性：**受限进程的
//! stdout 能否通过匿名管道接回父进程**——接不回来，工具执行（读命令输出）
//! 就不可用。
//!
//! 验证项：
//! 1. CreatePipe + STARTF_USESTDHANDLES 句柄继承在 CreateProcessAsUserW 下
//!    正常工作（同用户令牌，句柄继承不受 restrict SID 影响）；
//! 2. 从管道读回 `cmd /C echo` 的完整输出；
//! 3. `adb devices` 的真实输出可捕获（设备列表打印到本进程 stdout）。
//!
//! 结论按总纲 G3 章任务书记录。运行：`cargo run --bin sandbox_poc_v2`。

use std::io::Read;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    CreateRestrictedToken, GetTokenInformation, TokenElevation, DISABLE_MAX_PRIVILEGE,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_IMPERSONATE, TOKEN_QUERY,
};
use windows::Win32::System::Pipes::CreateNamedPipeW;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOW,
};
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
use windows::Win32::System::Pipes::{PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT};

fn wide(s: &str) -> Vec<u16> {
    std::os::windows::ffi::OsStrExt::encode_wide(std::ffi::OsStr::new(s))
        .chain(std::iter::once(0))
        .collect()
}

fn main() {
    println!("== G3 sandbox spike v2 (stdio bridge) ==");
    let restricted = unsafe { create_restricted_token() };
    println!("[1] CreateRestrictedToken: {}", !restricted.is_invalid());

    // 验证 2：echo 的输出经管道完整读回。
    match unsafe { spawn_capturing(&restricted, "cmd /C echo restricted-stdio-ok") } {
        Ok((exit_code, output)) => println!(
            "[2] echo captured: exit={exit_code} output={output:?}{}",
            if output.trim() == "restricted-stdio-ok" { " ✓完整" } else { " ✗不完整" }
        ),
        Err(error) => println!("[2] echo FAILED: {error}"),
    }

    // 验证 3：adb devices 真实输出捕获。
    match unsafe { spawn_capturing(&restricted, "adb devices") } {
        Ok((exit_code, output)) => {
            println!("[3] adb devices captured: exit={exit_code}");
            println!("    --- 设备列表（受限令牌下捕获）---");
            for line in output.lines() {
                if !line.trim().is_empty() {
                    println!("    {line}");
                }
            }
        }
        Err(error) => println!("[3] adb devices FAILED: {error}"),
    }

    println!("== v2 完成 ==");
}

unsafe fn create_restricted_token() -> HANDLE {
    let mut base = HANDLE::default();
    if OpenProcessToken(
        GetCurrentProcess(),
        TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_IMPERSONATE,
        &mut base,
    )
    .is_err()
    {
        return HANDLE::default();
    }
    let mut out = HANDLE::default();
    let _ = CreateRestrictedToken(base, DISABLE_MAX_PRIVILEGE, None, None, None, &mut out);
    let _ = CloseHandle(base);
    out
}

/// 用命名管道（inbound）捕获受限子进程的 stdout。命名管道比匿名管道少一步
/// 句柄继承的坑（子端以 `\\.\pipe\name` 打开），且与产品化的 stdio bridge
/// （codex stdio_bridge.rs 路线）同构。
unsafe fn spawn_capturing(token: &HANDLE, command: &str) -> Result<(u32, String), String> {
    if token.is_invalid() {
        return Err("无受限令牌".into());
    }
    let pipe_name = format!(
        "\\\\.\\pipe\\android-tool-g3-spike-{}",
        std::process::id()
    );
    let server = CreateNamedPipeW(
        PCWSTR::from_raw(wide(&pipe_name).as_ptr()),
        PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1,
        4096,
        4096,
        0,
        None,
    );
    if server == INVALID_HANDLE_VALUE || server.is_invalid() {
        return Err("CreateNamedPipeW 失败".into());
    }
    // 子进程命令：把 stdout 重定向到管道客户端端。
    let child_command = format!("{command} > \"\\\\.\\pipe\\android-tool-g3-spike-{}\"", std::process::id());
    let mut cmd = wide(&child_command);
    let mut si = STARTUPINFOW::default();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi = PROCESS_INFORMATION::default();
    let created = CreateProcessAsUserW(
        Some(*token),
        PCWSTR::null(),
        Some(windows::core::PWSTR(cmd.as_mut_ptr())),
        None,
        None,
        false,
        CREATE_UNICODE_ENVIRONMENT,
        None,
        None,
        &si,
        &mut pi,
    );
    if let Err(error) = created {
        let _ = CloseHandle(server);
        return Err(format!("CreateProcessAsUserW 失败: {error}"));
    }
    // 父端读管道直到子端关闭（ConnectNamedPipe 由子端 ConnectFile 隐式完成；
    // 若子进程没连上，ReadFile 在断开时返回 0 字节）。
    let mut output_bytes = Vec::new();
    {
        use std::os::windows::io::FromRawHandle;
        let mut server_read = std::fs::File::from(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(server.0 as _) });
        let _ = server_read.read_to_end(&mut output_bytes);
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    let mut exit_code = 0u32;
    let _ = GetExitCodeProcess(pi.hProcess, &mut exit_code);
    let _ = CloseHandle(pi.hProcess);
    let _ = CloseHandle(pi.hThread);
    Ok((exit_code, String::from_utf8_lossy(&output_bytes).trim_end().to_string()))
}

//! G3 spike：restricted token 降权 spawn 的最小可运行验证（POC）。
//!
//! 目的：验证「模型生成的命令以受限令牌运行」在 Windows 上可行，且
//! adb/USB 设备访问**不被隔断**（G3-C 被否的硬约束）。不接入产品——
//! 独立二进制，`cargo run --bin sandbox_poc`（在 src-tauri 下）。
//!
//! 验证项：
//! 1. CreateRestrictedToken 成功派生低权令牌（DISABLE_MAX_PRIVILEGE 剥离全部
//!    特权，含 SeDebugPrivilege）；
//! 2. 以该令牌 CreateProcessAsUser 启动 `adb devices`——USB 直通验证
//!    （adb 服务端以用户态运行，设备枚举不需要特权）；
//! 3. 受限令牌下普通命令可复用执行（令牌合法稳定）。
//!
//! 佐证参考：codex `windows-sandbox-rs/src/token.rs`（restricted token 组装）、
//! `spawn_prep.rs`（降权 spawn 流程）。这是 G3-A/B 任务书（总纲 G3 章）定义的
//! 「一周 spike」第一步——POC 结论决定自研/移植路线，不构成产品功能。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    CreateRestrictedToken, GetTokenInformation, TokenElevation, DISABLE_MAX_PRIVILEGE,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_IMPERSONATE, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

fn wide(s: &str) -> Vec<u16> {
    std::os::windows::ffi::OsStrExt::encode_wide(std::ffi::OsStr::new(s))
        .chain(std::iter::once(0))
        .collect()
}

fn main() {
    println!("== G3 sandbox spike ==");
    report_baseline();

    // 步骤 1：派生受限令牌（剥离全部特权）。
    let restricted = unsafe { create_restricted_token() };
    println!("[1] CreateRestrictedToken: {}", !restricted.is_invalid());

    // 步骤 2：以受限令牌启动 adb devices——USB 直通验证（G3-C 否决项）。
    match unsafe { spawn_as_user(&restricted, "adb devices") } {
        Ok(exit_code) => println!(
            "[2] adb devices under restricted token: exit={exit_code}（0=成功；插上设备复跑看枚举列表）"
        ),
        Err(error) => println!("[2] adb devices FAILED: {error}"),
    }

    // 步骤 3：受限令牌下再跑一个普通命令，验证令牌稳定可复用。
    match unsafe { spawn_as_user(&restricted, "cmd /C echo sandbox-poc-ok") } {
        Ok(exit_code) => println!("[3] plain cmd under restricted token: exit={exit_code}"),
        Err(error) => println!("[3] plain cmd FAILED: {error}"),
    }

    println!("== spike 完成：结论按总纲 G3 章任务书记录 ==");
}

fn report_baseline() {
    unsafe {
        let token = GetCurrentProcess();
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let _ = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        println!(
            "[0] 当前进程令牌提权级别: {}",
            if elevation.TokenIsElevated != 0 { "elevated" } else { "not elevated" }
        );
    }
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
    // DISABLE_MAX_PRIVILEGE：剥离全部特权（含 SeDebugPrivilege）。生产版按
    // codex token.rs 保留最小集并配置 restrict SID；POC 先验证最严形态。
    let mut out = HANDLE::default();
    let result = CreateRestrictedToken(
        base,
        DISABLE_MAX_PRIVILEGE,
        None,
        None,
        None,
        &mut out,
    );
    let _ = CloseHandle(base);
    if result.is_err() {
        return HANDLE::default();
    }
    out
}

unsafe fn spawn_as_user(token: &HANDLE, command: &str) -> Result<u32, String> {
    if token.is_invalid() {
        return Err("无受限令牌".into());
    }
    let mut cmd = wide(command);
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
        // ERROR_ACCESS_DENIED 通常源于交互式桌面会话的 window station ACL
        // 未对受限令牌授权——这正是产品化需要 launch_environment + stdio
        // bridge（codex 路线）的地方；POC 记录信号即可。
        return Err(format!("CreateProcessAsUserW 失败: {error}"));
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    let mut exit_code = 0u32;
    let _ = GetExitCodeProcess(pi.hProcess, &mut exit_code);
    let _ = CloseHandle(pi.hProcess);
    let _ = CloseHandle(pi.hThread);
    Ok(exit_code)
}

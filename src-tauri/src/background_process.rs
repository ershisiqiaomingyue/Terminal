use std::{ffi::OsStr, process::Command};

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for (name, value) in agent_core::child_proxy_environment() {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::process::Stdio;

    /// 探针 C#：只做被测的那一件事——取本进程的 console window 句柄。
    const PROBE_CS: &str = "using System; using System.Runtime.InteropServices; public static class ConsoleProbe { [DllImport(\"kernel32.dll\")] public static extern IntPtr GetConsoleWindow(); }";

    /// 编译探针的 PowerShell 脚本。
    ///
    /// 不用 `Add-Type -TypeDefinition` 的原因（本机 Windows PowerShell 5.1 实测）：由测试进程
    /// 派生的 PowerShell 里，Add-Type 的编译链路必然失败并抛 `System.ComponentModel.Win32Exception`
    /// （native error 1314，"A required privilege is not held by the client"），
    /// 与脚本怎么写无关：`-TypeDefinition`、`-Path`、经 stdin 传入都是同一个 1314，
    /// 而 `Add-Type -AssemblyName`（只加载不编译）与直接调用 csc.exe 都正常。
    /// 因此这里显式用 csc.exe 编译、再 `Assembly.LoadFrom` 加载，绕开 Add-Type 的编译链路。
    ///
    /// 脚本经 stdin 交给 `-Command -`，不经过命令行参数的重新解析，
    /// 所以 C# 里的双引号不会被 Windows 的命令行解析吃掉。
    fn probe_script(work_dir: &std::path::Path) -> String {
        let source = work_dir.join("console_probe.cs");
        let assembly = work_dir.join("console_probe.dll");
        format!(
            "$csc = Join-Path $env:windir 'Microsoft.NET\\Framework64\\v4.0.30319\\csc.exe'\n\
             if (-not (Test-Path $csc)) {{ $csc = Join-Path $env:windir 'Microsoft.NET\\Framework\\v4.0.30319\\csc.exe' }}\n\
             $log = & $csc /nologo /target:library /out:'{assembly}' '{source}' 2>&1\n\
             if ($LASTEXITCODE -ne 0) {{ [Console]::Error.Write('csc failed: ' + ($log -join ' | ')); exit 2 }}\n\
             [void][System.Reflection.Assembly]::LoadFrom('{assembly}')\n\
             [Console]::Out.WriteLine([ConsoleProbe]::GetConsoleWindow().ToInt64())\n\
             [Console]::Error.Write('stderr-ok')\n",
            assembly = assembly.display(),
            source = source.display(),
        )
    }

    #[test]
    fn background_child_has_no_console_and_retains_output() {
        let work_dir = std::env::temp_dir().join(format!("androidtool_console_probe_{}", std::process::id()));
        std::fs::create_dir_all(&work_dir).expect("probe work dir should be created");
        std::fs::write(work_dir.join("console_probe.cs"), PROBE_CS).expect("probe source should be written");

        let mut child = command("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("background process should start");
        child.stdin.take().expect("probe stdin should be piped")
            .write_all(probe_script(&work_dir).as_bytes()).expect("probe script should be written to stdin");
        let output = child.wait_with_output().expect("background process should finish");
        std::fs::remove_dir_all(&work_dir).ok();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "probe exited with {}: {}", output.status, stderr);
        assert_eq!(stdout.trim(), "0", "probe stdout: {:?}, stderr: {:?}", stdout, stderr);
        assert_eq!(stderr, "stderr-ok");
    }
}

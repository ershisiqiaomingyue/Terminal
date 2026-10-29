# 运行 Rust 单元测试。
#
# 为什么不能直接 `cargo test --lib`：测试可执行文件与桌面应用链接同一份代码，
# 其中 tauri-plugin-dialog 引入的 rfd 会导入 comctl32 v6 专属导出（TaskDialogIndirect
# 等）。测试二进制没有内嵌应用清单，Windows 就绑定 comctl32 5.82，加载器在 main()
# 之前以 STATUS_ENTRYPOINT_NOT_FOUND（0xC0000139）结束进程，所有测试零输出即死。
#
# 修法：先构建测试二进制，再把它与 `src-tauri/windows-test.manifest`（改名为
# `<exe>.manifest`）复制到临时目录里执行——外部清单只对没有内嵌清单的 exe 生效；
# 换目录执行还顺带避开了 Windows 对同名 exe 的清单缓存。
#
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-rust.ps1
#   额外参数会透传给测试二进制，例如：-- --test-threads=1
[CmdletBinding()]
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$TestArgs)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root 'src-tauri\windows-test.manifest'
if (-not (Test-Path -LiteralPath $manifest)) { throw '找不到测试清单' }

Push-Location $root
try {
    Write-Host '==> cargo test --lib --no-run' -ForegroundColor Cyan
    & cargo test -p androidtoolkit-desktop --lib --no-run
    if ($LASTEXITCODE -ne 0) { throw '构建测试二进制失败' }

    $deps = Join-Path $root 'target\debug\deps'
    $exe = Get-ChildItem -LiteralPath $deps -Filter 'androidtoolkit_desktop_lib-*.exe' -File |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $exe) { throw '没找到测试可执行文件' }

    $sandbox = Join-Path $env:TEMP ('terminal-rust-tests-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $sandbox | Out-Null
    $target = Join-Path $sandbox $exe.Name
    Copy-Item -LiteralPath $exe.FullName -Destination $target -Force
    Copy-Item -LiteralPath $manifest -Destination ($target + '.manifest') -Force

    Write-Host '==> 运行测试' -ForegroundColor Cyan
    & $target @TestArgs
    exit $LASTEXITCODE
}
finally { Pop-Location }

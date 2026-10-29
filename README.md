# Terminal

独立桌面终端应用（Tauri 2 + React + TypeScript），从 `../Android-Tool` 的**终端工作台**整体移植而来。

## 界面

整个应用只有一个界面 —— 终端工作台：

- **左栏**：新终端入口（PowerShell / cmd / WSL / SSH / ADB）、“已打开的终端”、“已保存服务器”（SSH 与 WSL 配置）。
- **中栏**：多面板终端，支持 1x1 / 1x2 / 上下 / 2x2 分栏，面板可拖拽互换、可拆成独立窗口。
- **右栏**：主机状态（CPU / 内存 / 磁盘 / 网速 / 监听端口 / 进程）、Git 变更、文件浏览与上传下载。
- **底部**：命令输入框，以及“用自然语言生成命令”的 AI 命令栏。

终端支持本地 Shell（PowerShell / pwsh / cmd）、WSL 发行版、SSH 远程主机、ADB Shell 四类会话；
SSH 走 russh 主引擎 + ssh.exe 兜底，WSL / SSH 的文件面板共用同一套代码查看器链路。

## 目录

```
src/                 前端（React + TS）
  views/Terminal/    终端工作台（界面本体）
  views/CodeViewer/  右栏文件面板与代码预览
  platform/          终端内核、SSH/WSL 数据源、设置存储、主题
  components/        通用 UI 与设置弹层
src-tauri/           Rust 后端（Tauri 2）
  src/terminal.rs    PTY / russh 会话
  src/ssh.rs         SSH 连接管理与凭据（keyring）
  src/wsl.rs         WSL 发行版与状态
  src/code_viewer.rs 文件浏览 / Git / 上传下载
crates/agent-core    SSH 传输（russh / libssh2）、资源探针等底层实现
crates/android-core  ADB 相关底层实现
```

## 开发

```bash
pnpm install
pnpm dev                 # 仅前端（Vite :1420）
cargo build -p androidtoolkit-desktop   # 后端（debug）
```

启动桌面应用：先 `pnpm dev`，再运行 `target/debug/androidtoolkit-desktop.exe`（debug 构建会连 Vite 的 devUrl）。

完整打包（会自行执行前端构建）：

```bash
pnpm build
cargo build --release -p androidtoolkit-desktop
```

## 测试

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-rust.ps1
```

该脚本先构建测试二进制，再连同 `src-tauri/windows-test.manifest` 复制到临时目录执行。
直接 `cargo test --lib` 在 Windows 上会因测试二进制缺少 comctl32 v6 清单而在 main() 之前
被杀掉（`STATUS_ENTRYPOINT_NOT_FOUND`），所以必须走脚本。

前端类型检查与构建：`pnpm exec tsc --noEmit`、`pnpm build`。

## 与 Android-Tool 的关系

- 本项目的代码是从 `../Android-Tool` **复制**而来的，没有修改源项目的任何文件。
- 两者使用相同的应用标识（`com.androidtoolkit.desktop`），因此 **SSH 连接、WSL 配置、主题等设置是共用的**：
  在任一应用里新增的服务器，另一边也能直接看到。

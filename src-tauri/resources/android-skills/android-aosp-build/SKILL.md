---
name: android-aosp-build
description: 验证 AOSP 源码环境、执行指定 lunch 目标和模块构建，分析编译日志中的首个根因及后续错误。
metadata:
  runtime: instruction
---

# AOSP 构建与错误诊断

- 使用用户指定的 SSH/WSL 连接和源码根目录；连接 ID 必须来自现有配置，workspaceRoot 必须是实际路径。缺少构建目标或模块时先检查环境，不能擅自选择产品或启动整树编译。
- 用 open_application_tool 的 open.tool.aosp 交接，parameters.operation=inspect 验证源码树；用户要求编译且参数明确时 operation=build。其余参数为 targetKind（ssh/wsl）、targetId、workspaceRoot、lunchTarget、moduleName，均使用字符串。
- 页面负责环境校验和真实构建；handed_off 不是完成。用 query_android_tasks 获取对应 aosp-build 任务的目标、状态、错误和日志，再用 query_android_aosp_build 读取有限错误/警告证据。
- 从首个有上下文的编译器、Soong、Ninja、链接或缺依赖错误定位根因；后续级联报错和最终 exit code 不一定是根因。保留模块名、文件位置和关键诊断原文。
- 建议改动时区分源码错误、构建配置、工具链和资源不足。用户要求修改后才编辑或重建；不要以 rm -rf out、全量 clean、更新依赖或关闭 SELinux 作为默认修复。
- 若需要用户选目标或连接，说明等待哪项配置；不要无间隔轮询任务，不把旧构建记录当成本次结果。

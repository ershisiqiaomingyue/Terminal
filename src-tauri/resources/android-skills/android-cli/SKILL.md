---
name: android-cli
description: 检查 Android 设备连接、ADB 授权及设备状态，读取电池、温度、内存、CPU、图形或网络诊断，定位设备异常。
metadata:
  runtime: instruction
---

# Android 设备诊断

设备连接、授权或系统状态异常时采用此工作流；用户也可用 /android-cli 指定。

- 用 query_android_device 列出真实设备。用户给出序列号时保持该目标；未指定且只有一个在线设备可直接使用；多个在线设备时确认目标。offline、unauthorized 和无设备是连接问题，不能据此断言应用故障。
- 选定 serial 后查询概览；按问题选择 report：battery、battery-stats、thermal、memory、cpu、activities、network、graphics、surfaceflinger 或 performance-support。不要一次读完所有报告。
- 根据返回证据区分系统压力、应用异常和依赖缺失；记录设备、报告来源、时间范围以及数据是否截断。不能从瞬时 CPU 或内存快照直接推断持续故障。
- 用户要求刷新连接时调用 open_application_tool，action_id=open.tool.adb；需要手动 Shell 时用 open.tool.shell 预填 command，终端仍需用户确认发送。不能把预填命令说成已经执行。
- 重启、安装、清除数据等只按用户要求执行，优先交给对应设备或应用页面。当前日志检查使用 capture_android_logcat，不用历史日志替代现场。
- 若当前执行环境没有 Android 工具，说明需要本地 Windows/ADB 环境，不改用远程 SSH 的 adb 冒充本地设备。

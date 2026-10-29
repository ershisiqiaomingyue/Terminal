---
name: android-task-guidance
description: 当前任务涉及 Android、AOSP 或 Gradle 工程的源码、构建、设备或日志操作时，提供通用约定；具体诊断流程按需选用其他专项 Skill。仅出现项目名或讨论界面样式不属于适用条件。
runtime: instruction
---

# Android 任务通用指导

仅用于当前用户要求的相关工作。以当前项目文件、用户指导和工具证据为准，不假定项目类型、构建目录、产品目标或构建类型，不套用示例工程和固定构建命令。

- 需要连续的环境初始化与构建时保持在同一 shell；缺少用户必须选择的产品或目标时使用询问工具，不猜测。
- 当前设备日志可使用 capture_android_logcat，按任务合并关键词；query_android_logcat 用于指定的历史记录。没有新复现或扩大范围的要求时，不重复读取同一份日志。
- 当本轮提供专用 Android API 时，归档读取可用 android_archive_read，编辑构建用 android_archive_edit；设备查询用 android_adb_read，修改设备用 android_adb_action。仅使用实际提供的工具和参数，直接在对话报告真实结果；仅当用户要操作页面时导航到相应页面。
- 设备 shell 的命令属于设备端，主机路径属于对应主机。诊断注明实际来源和范围，不从结构摘要推断没有证据的泄漏对象或性能原因。
- SELinux 等专项 Skill 的检索范围遵循已配置来源，按需要加载对应 Skill、定位相关文件并汇报证据；不扫描无关目录，不要求固定的 Android 目录布局。

本说明不改变当前权限、执行主机或用户任务范围。

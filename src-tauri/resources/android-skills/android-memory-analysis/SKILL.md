---
name: android-memory-analysis
description: 调查 Android 内存增长、OOM 和 Java 泄漏，结合设备内存报告、HPROF 任务及 Shark 对象图证据分析。
metadata:
  runtime: instruction
---

# Android 内存分析

- 先确定设备、包名、复现步骤和观测时间；使用 query_android_device 的 memory 报告检查现场内存压力。单个快照不能证明泄漏，需要可比较的阶段数据。
- 已有堆转储先 query_android_tasks，使用对应完成任务调用 query_android_heap_dump。结构摘要只说明 HPROF 是否可识别及记录构成，不能据此宣称找到泄漏或计算 retained size。
- 用户需要新转储时用 open_application_tool 的 open.tool.heap-dump，parameters 为 serial、packageName、destination（已知本地输出目录）。页面负责选择、采集与分析；等待实际任务完成再读取。
- 已有 Shark 分析任务时，将真实 analysisTaskId 与堆转储任务 ID 一起交给 query_android_heap_dump，依据返回对象图、GC root 路径和分析限制判断。没有对象图结果时说明需在堆转储页面完成分析。
- 区分 Java 堆、Native 内存、图形内存和系统内存压力。OOM 不等于泄漏；给出对象保留证据、前后变化与验证步骤，避免把方法或类数量当成内存大小。
- 不自动清除应用数据、杀进程或重启设备；这些行为会破坏复现现场，只有用户要求时才交给对应页面执行。

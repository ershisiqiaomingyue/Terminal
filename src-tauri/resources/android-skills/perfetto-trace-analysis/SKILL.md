---
name: perfetto-trace-analysis
description: 分析 Perfetto Trace 中的卡顿、启动耗时、CPU 和线程调度；必要时采集 Trace，依据只读查询定位性能瓶颈。
metadata:
  runtime: instruction
---

# Perfetto 性能分析

用户调查卡顿、启动、CPU 或调度问题且涉及 Trace 时采用；也可用 /perfetto-trace-analysis 指定。

- 有用户提供的 Trace 就优先分析；已有采集任务先用 query_android_tasks 获取实际产物路径和完成状态。不要猜文件名或复用另一个设备的任务。
- 需要新采集时先用 query_android_device 的 performance-support 确认能力，按用户要求调用 open_application_tool 的 open.tool.perf，parameters 中使用 serial、durationSeconds（1 到 300 的整数字符串）、destination（已知本地目录）。缺目录交给页面选择；不得用自然语言代替路径。
- 工具返回 handed_off 只表示页面收到请求。用 query_android_tasks 确认对应任务完成后再查询产物；未完成时说明等待，不反复无间隔轮询。
- 用 query_android_perfetto 的 tracePath 和 mode=diagnose 查看可用诊断，再围绕具体疑点用 mode=sql 查询单条只读 SELECT/WITH。依据实际存在的表调整查询，缺表表示该证据不可用。
- 指出进程/线程、时间区间、Runnable、CPU 或帧证据及其单位；区分相关性和原因。输出截断时缩小范围，不能编造丢帧率和耗时。
- 若需要 Native 热点或 Java 方法采样，使用相应采集页面和 query_android_simpleperf / query_android_method_profile。方法索引不等于热点耗时，结构摘要不足时说明还需分析。

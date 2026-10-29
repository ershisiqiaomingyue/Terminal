---
name: android-crash-analysis
description: 分析 Android 应用闪退、ANR、Java 异常或 Native 崩溃，关联 Logcat、诊断产物和符号化结果定位故障。
metadata:
  runtime: instruction
---

# Android 崩溃与 ANR 分析

- 先确定包名、设备和复现时段。设备不明确时用 query_android_device 列出在线目标；只有一个在线设备可使用它，多个设备请确认。
- 调查当前故障用 capture_android_logcat 在后台读取当前设备缓冲区，参数为 serial、terms、matchMode、limit、maxLines、timeoutMs。候选异常词合并一次筛选，matchMode=any 表示任一匹配，all 仅用于同一行同时满足条件。不要对每个关键字重复采集。
- 包名、PID、tag 和复现时段用于核对返回日志的上下文；当前采集工具没有专用的包名/PID/tag/起止时间参数，不能虚构这些字段或声称已做精确时间过滤。maxLines 仅限制最近读取行数。范围不足时说明缺口；需要持续观察时由用户在 Logcat 页面采集。
- 仅当用户要已保存日志时用 query_android_logcat，先获取会话再按 sessionId 查询。用户已选本地日志或压缩包时使用 search_local_logs 和 read_local_log_range 读取匹配位置的上下文，source/member 必须来自真实结果。
- Java 崩溃关联 FATAL EXCEPTION、进程、异常类型、Caused by 和应用栈；ANR 结合原因、主线程状态、锁持有者及对应时间段。看到 Exception 字样不足以认定应用闪退。
- 对已完成的诊断或 bugreport 任务，先 query_android_tasks 再 query_android_diagnostics / query_android_bugreport，取对应文件的有限证据。
- Native 崩溃结合信号、fault address、线程和 backtrace；用户提供 tombstone 及匹配构建的符号目录时用 open_application_tool 的 open.tool.native-symbolizer（tombstonePath、symbolsPath），任务完成后 query_android_native_symbolizer。符号版本不匹配时不能给出确定源码行。
- 交接不代表完成。总结直接证据、可能原因和下一步验证；缺复现日志或调用栈时明确缺口，不能编造结论。

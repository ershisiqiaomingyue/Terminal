---
name: android-apk-analysis
description: 检查 APK/JAR 的清单、权限、包结构及签名构建记录，调查安装失败、包差异或应用配置问题。
metadata:
  runtime: instruction
---

# APK 与包结构分析

- 先使用用户选择或搜索得到的实际本地 APK/JAR 路径；不要把“桌面的安装包”等描述当成路径，也不猜包名或版本。
- 用户要求解析包时用 open_application_tool 的 open.tool.archive，parameters.path 为实际路径。页面读取包结构和清单；没有路径时交给页面选择文件。
- 通过 query_android_tasks 的完成记录、query_android_archive_history 的包元数据和工作区记录读取结果；交接不代表解析、签名或安装成功。
- 结合真实清单检查 package、version、SDK、组件导出属性及权限。单凭声明权限不能认定应用实际访问了数据；未知签名或缺证据时不能断言签名一致。
- 安装失败结合任务/操作审计中的具体错误、设备版本、已安装包和签名信息；需要设备状态可 query_android_device，需要定位已安装包可交接 open.tool.apps（serial、packageName）。
- 仅按用户要求修改、重签名、安装或卸载；分析任务不自动改包。比较两个版本时确保比较的是用户指定的两个实际文件或记录，并标明每项证据来源。

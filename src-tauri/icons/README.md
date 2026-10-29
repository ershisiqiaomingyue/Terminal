# 应用品牌图标

当前统一使用 `aaaa/README.md` 定稿的 **C 方案：水银天青冷光（Liquid Mercury Azure）**。

- 原始设计：`aaaa/icon-v2-mercury.svg`，与当前的 `aaaa/icon.svg` 一致。
- 应用内部：`src/assets/brand-icon.svg`，保留原始矢量图形，供启动加载页、关于面板和网页标签共用。
- 系统源图：本目录的 `source.svg`，保留原始 C 方案，用于 macOS、Windows Store、Android 和 iOS 资源。
- Windows 小尺寸版：`taskbar.svg` 保留原始几何和配色，将画布收紧到 `64 64 384 384`、移除外围氛围光晕并减轻投影。对应应用 PNG、`icon.ico` 和 `installer.ico`，程序、快捷方式、任务栏及安装程序统一使用透明 C 方案。

在项目根目录执行：

```powershell
npm run icons:generate
```

该命令通过项目已有的 Tauri CLI 生成各平台资源，再应用 Windows 小尺寸版本，不需要额外图像依赖。

`tauri.conf.json` 引用应用 PNG、ICO 和 ICNS，NSIS 安装程序引用 `installer.ico`。Windows 窗口使用 Tauri 从 ICO 生成的默认图标；启动完成后不再用 512px PNG 覆盖它。系统图标随桌面程序重新构建生效。

切换设计时需同步更新 `src/assets/brand-icon.svg`、`source.svg` 和 `taskbar.svg`，再运行生成命令。不要直接向本目录运行 `tauri icon source.svg`，以免覆盖 Windows 小尺寸版本。

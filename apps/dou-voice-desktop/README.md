# Dou Voice Desktop

这是 Dou Voice 的桌面应用包，包含 Tauri Rust 后端、React 主窗口、React overlay 和静态前端资源。

## 运行

仓库根目录运行：

```powershell
cargo run -p dou-voice-desktop
```

预期结果：

- 打开 `Dou Voice` 主窗口。
- 首次运行且没有 `settings.json` 时显示 setup wizard。
- 系统托盘显示 Dou Voice 图标。
- 关闭主窗口或登录窗口时窗口隐藏，进程继续常驻托盘。

## 前端构建

主窗口和 overlay 都由 `build-web.js` 使用 Bun 构建。通常不需要手动执行，因为 `src-tauri/build.rs` 会在 Cargo build 阶段自动安装依赖并运行前端构建。

手动构建：

```powershell
bun run build:web
```

只构建 overlay：

```powershell
bun run build:overlay
```

生成的 `web/scripts/main-react.js` 和 `web/scripts/overlay-react.js` 是构建产物，已在 `.gitignore` 中忽略。

## 首次使用流程

首次启动时，如果用户配置文件不存在，应用显示 setup wizard：

1. `Doubao Session`：打开豆包登录窗口，登录后导出 `auth.json`。
2. `Input Basics`：选择麦克风、文本插入方式、overlay 和 sound。
3. `Press To Talk`：保留默认 `Ctrl+Q` 或捕获新热键。
4. `Ready Check`：运行一次 `Test Recording`，确认链路可用，然后 `Finish Setup`。

完成 wizard 后，主窗口提供四个分区：

- `General`：实时状态、热键、麦克风、输入方式、音效和 overlay。
- `Auth`：认证状态、认证文件路径、登录窗口和认证导出。
- `Diagnostics`：最近活动日志和脱敏诊断导出。
- `About`：当前运行配置摘要。

## 认证文件

认证文件路径由 Tauri app config 目录决定，文件名固定为 `auth.json`。GUI 只展示路径，不支持手动改路径。

导出内容包括：

- 豆包相关 Cookie。
- `device_id`。
- `web_id`。
- 导出时间戳。

这些值用于 ASR WebSocket 连接，属于敏感本地数据，不应提交到仓库。

## 语音输入

默认日常入口是 press-to-talk：

1. 把光标放到目标输入框。
2. 按住当前热键开始录音。
3. 松开热键后停止麦克风输入，并等待 ASR final 结果。
4. 应用按设置把文本直接输入或写入剪贴板。

`Test Recording` 是固定 5 秒测试入口，适合验证麦克风、认证、ASR 和文本输入是否正常。

## 文本插入方式

- `Direct typing with fallback`：先使用平台直接输入。Windows 主路径是 `SendInput(KEYEVENTF_UNICODE)`；失败后自动使用剪贴板和粘贴快捷键 fallback。
- `Clipboard paste`：只把识别文本写入剪贴板，不发送粘贴快捷键。用户需要手动粘贴。

## Overlay 和托盘

overlay 是 416x112 的状态浮层，显示录音、识别状态、实时文本和麦克风波形。Windows/Linux 使用 Tauri 透明置顶窗口；macOS 使用 `tauri-nspanel` 创建 non-activating NSPanel。

托盘菜单包含：

- `Show Window`
- `Quit`

双击托盘图标会显示主窗口。托盘 tooltip 和图标状态会跟随录音、识别、输入和错误状态更新。

## 打包

进入本目录运行：

```powershell
cargo tauri build
```

当前 `tauri.conf.json` 的产品名是 `DouVoice`，窗口标题是 `Dou Voice`，应用标识是 `dou.voice`，Windows bundle 目标是 NSIS。

## 相关文档

- [根 README](../../README.md)
- [Windows 使用与排障](../../docs/windows-desktop.md)
- [开发与验证](../../docs/development.md)
- [架构说明](../../docs/architecture.md)

# Windows 使用与验收

本文档专注 Windows 平台的运行验收和排障。初次上手和使用方式见根 [README](../README.md)，开发命令、CI、打包见 [开发与验证](development.md)，代码组织见 [架构说明](architecture.md)。

## 环境要求

- Windows 10 / 11。
- Windows 原生 Rust stable toolchain（workspace 要求 `rust-version = "1.78"`）。
- Bun。
- WebView2 Runtime（Windows 11 通常已内置；Windows 10 可能需要手动安装）。
- 可用麦克风。
- 可登录豆包 Web。

## 必须使用 Windows 原生 toolchain

不要用 WSL2 toolchain 验证 Windows 桌面行为。Tauri 桌面构建路径与 Linux 不同，且输入模拟、托盘、overlay 行为只在 Windows 原生 toolchain 下被验证。

判断方法：如果 `cargo` 路径、项目路径或构建输出包含 `/mnt/d/...`，说明你在 WSL2 中。切到 Windows 原生 PowerShell，项目路径使用 `D:\Code\...` 形式。

### RustRover 配置

- Project 路径使用 Windows 路径，例如 `D:\Code\dou-voice`。
- Toolchain 指向 Windows 的 `cargo.exe` / `rustc.exe`，通常在 `%USERPROFILE%\.cargo\bin`。
- Run Configuration 执行 `cargo run -p dou-voice-desktop`。

## 启动

```powershell
cargo run -p dou-voice-desktop
```

预期：

- 出现 `Dou Voice` 主窗口。首次运行（没有 `settings.json`）进入 setup wizard。
- 系统托盘出现 Dou Voice 图标。
- 关闭主窗口或登录窗口时窗口隐藏，进程继续常驻托盘。从托盘菜单 `Quit` 完全退出。

## 首次配置

setup wizard 四步：

1. `Doubao Session`：`Open Login` → 登录豆包 → `Export Auth` → `Refresh` 确认认证可用。
2. `Input Basics`：选择麦克风、文本插入方式、overlay、sound。
3. `Press To Talk`：保留默认 `Ctrl+Q` 或捕获新热键。
4. `Ready Check`：`Test Recording` 运行 5 秒测试录音，成功后 `Finish Setup`。

认证文件：`%APPDATA%\dou.voice\auth.json`（含 Cookie、`device_id`、`web_id`，敏感，不要外泄）。
设置文件：`%APPDATA%\dou.voice\settings.json`。

## 日常使用

1. 光标放到目标输入框。
2. 按住热键（默认 `Ctrl+Q`）开始录音。
3. 说话时 overlay 显示状态、实时文本、麦克风波形。
4. 松开热键，应用停止录音，等待 ASR final。
5. 识别完成后，文本按设置输入或写入剪贴板。

热键实现细节：

- 30ms press debounce，避免重复 pressed 事件。
- busy 抑制，上一段还在识别或输入时忽略新按下。
- 30 秒 release fallback，避免极端情况下丢失 release 后一直录音。
- Windows 使用 30ms 间隔轮询 `dou-voice-platform::hotkey::hotkey_pressed`，因为部分 modifier-only 组合 Tauri global-shortcut 注册路径不支持。

## 设置项

- **Hotkey**：默认 `Ctrl+Q`。Windows 支持两个及以上修饰键组成的组合。
- **Microphone**：默认 `Default`。指定设备不可用时回退到系统默认输入设备。
- **Keep Microphone Ready**：默认关闭。开启后常开的是本地 CPAL 输入流；未按热键时 PCM 立即丢弃，只有按住热键的音频进入当次 ASR 会话。
- **Input Method**：
  - `Direct typing with fallback`：先 `enigo.text()`（等价 `SendInput(KEYEVENTF_UNICODE)`）；失败后备份剪贴板 → 写入文本 → 发送 Ctrl+V（扫描码 `Key::Other(0x56)` 绕过键盘布局）→ 还原剪贴板。
  - `Clipboard paste`：只写入剪贴板，不自动粘贴。
- **Sound**：Windows 播放内嵌短 WAV，反馈 Start / Stop / Complete / Error。
- **Overlay**：Tauri 透明置顶窗口，416×112，底部居中，底边距 56px，不应抢占焦点。

## 诊断

主窗口 `Diagnostics` 页查看最近活动日志，并可导出诊断 JSON 到：

```text
%APPDATA%\dou.voice\diagnostics\
```

诊断 JSON 包含：app 版本（含 git commit）和平台、auth 摘要（路径、是否存在、cookie 数量、device/web id 是否存在）、ASR 摘要（endpoint、origin、流式参数）、当前 voice status、最近 2000 条事件。不包含 Cookie / `device_id` / `web_id` 原文，可以安全附在 issue 里。

## 打包

```powershell
cd apps\dou-voice-desktop
cargo tauri build
```

或只出 NSIS：

```powershell
cargo tauri build --bundles nsis
```

Windows bundle 目标是 NSIS，`installMode = "currentUser"`。输出目录：

```text
apps\dou-voice-desktop\src-tauri\target\release\bundle\nsis\
```

## Windows 验收清单

1. 启动应用并完成 setup wizard。
2. `Auth` 页显示认证状态可用。
3. `General` 页能列出麦克风设备。
4. 在 Notepad 中按住 `Ctrl+Q` 说话，松开后文本只输入一次。
5. 录音时 overlay 显示麦克风波形，Notepad 仍保持焦点。
6. `Input Method` 切换到 `Clipboard paste` 后，识别文本进入剪贴板且不自动粘贴。
7. 关闭 `Overlay` 后再次录音，不显示 overlay，但托盘、音效、日志和输入仍工作。
8. 导出的诊断文件不包含敏感原文。
9. `cargo tauri build` 能生成 NSIS 安装包。

## 常见问题

### 构建时报缺少 atk / pango / gtk / webkit2gtk

这是 Linux / WSL 构建路径的依赖，不是 Windows 原生桌面路径。切换到 Windows 原生 `cargo.exe` 并使用 `D:\...` 形式项目路径。

### 登录后导出的 Cookie 很少或认证不可用

确认登录窗口仍打开且页面已经登录。点击 `Export Auth` 后再点 `Refresh`。如果仍不可用，关闭登录窗口重新打开并重新登录，不要手工编辑 Cookie。

### 热键录音后没有输入文本

打开 `Diagnostics` 页查看活动日志：

- 没有 `asr_final` 或 `final_text` → 优先检查认证、麦克风和网络。
- 有 `final_text` 但没有输入 → 查看是否有 `input_fallback` 或 `failed to type text`。
- 选择了 `Clipboard paste` → 需要手动 `Ctrl+V`。
- 目标程序以管理员权限运行 → 普通权限应用的输入模拟可能被系统拦截，尝试用管理员权限运行 Dou Voice。

### Overlay 不显示

确认 `General` 页 `Overlay` 已开启。idle 状态且没有最新文本时 overlay 会隐藏；错误或 idle 状态会延迟隐藏（`OVERLAY_HIDE_DELAY = 1.6s`）。

### 关闭主窗口后应用仍在运行

这是预期行为。关闭主窗口只隐藏窗口，进程常驻托盘。从托盘菜单 `Quit` 完全退出。双击托盘图标可以重新打开主窗口。

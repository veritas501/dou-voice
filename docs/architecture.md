# 架构说明

本文档面向「想理解代码组织、定位问题、改某个链路」的开发者。运行/打包/CI 见 [开发与验证](development.md)，Windows 平台行为见 [Windows 使用与验收](windows-desktop.md)。

## 总体分层

```text
┌─────────────────────────────────────────────────────────────┐
│ apps/dou-voice-desktop (Tauri shell)                        │
│   前端（React） + src-tauri（命令、托盘、热键、overlay、worker）│
└───────────────┬─────────────────────────────────────────────┘
                │
┌───────────────┴─────────────────────────────────────────────┐
│ crates/dou-voice-platform                                   │
│   文本输入 / 热键解析 / 反馈音（按 cfg 切换平台子模块）        │
└───────────────┬─────────────────────────────────────────────┘
                │
┌───────────────┴─────────────────────────────────────────────┐
│ crates/dou-voice-core                                       │
│   ASR 协议 / 音频采集 / 认证存储 / 状态机 / 错误模型          │
└─────────────────────────────────────────────────────────────┘
```

依赖方向自上而下：Tauri shell 依赖 platform 和 core，platform 依赖 core，core 不依赖任何上层。OS 相关能力一律封进 `dou-voice-platform`，不泄漏到 core；Tauri shell 只做产品集成，不实现核心协议。

## crates/dou-voice-core

跨平台核心，不含任何 OS 细节。子模块：

- `asr/`：豆包 ASR WebSocket 协议。
  - `config.rs`：`AsrClientConfig`，构造 ASR URL，按 `wire_protocol()` 切换 VoiceGenie 和 legacy samantha 协议参数（`api_app_key`、`namespace`、`aid=497858`、`device_id`、`web_id`、`web_tab_id` 等）。
  - `mod.rs`：`transcribe_pcm_bytes` / `transcribe_pcm_stream` / `transcribe_pcm_stream_with_events`，WebSocket 建连、Cookie header、PCM 上传、事件收集。
  - `event.rs`：`AsrEvent` 枚举（`Opened` / `Partial` / `Final` / `Finished` / `AuthExpired` / `Error` / 超时与关闭等），`transcript_text_from_events` 在没有 `Final` 时用 `Partial` 兜底。
  - `session.rs` / `send.rs` / `receive.rs` / `parser.rs` / `protocol.rs` / `options.rs`：session 建立、PCM 发送、事件接收、二进制与服务端事件解析、协议常量、`PcmTranscribeOptions`。
- `audio/`：`AudioFormat`（`PCM_16K_MONO` 常量）、`PcmChunk`、`InputDeviceInfo`、`record_input` / `start_input_streaming` 等 CPAL 录音封装。
- `auth/`：`AuthParams`（cookies、device_id、web_id、captured_at_unix_ms）、`AuthParamsStore`，`is_complete` / `validate` / `cookie_header`。`json.rs` 负责 `auth.json` 读写。
- `state/`：`TranscriptionState`（`Idle` / `Starting` / `Recording` / `Stopping`）、`TranscriptionCommand`（`Toggle` / `Connected` / `StopAccepted` / `Finished` / `Failed`）、`TranscriptionStateMachine`。状态机只描述录音/识别生命周期，不感知 UI。
- `error/`：`CoreError`（`Io` / `Json` / `InvalidState` / `MissingAuth` / `InvalidAuth` / `AudioUnavailable` / `AsrConnection` / `AuthExpired`）和 `CoreResult<T>`。错误文案稳定英文，供 UI 直接展示。

## crates/dou-voice-platform

按 `cfg` 切换平台实现的适配层。三个子模块各自暴露统一函数：

- `input.rs`：`type_text` / `copy_text_to_clipboard`，`TextInputMethod`（`Direct` / `Clipboard`）、`TextInputOutcome`。
  - 主路径：`enigo.text()`（Windows 等价 `SendInput(KEYEVENTF_UNICODE)`，macOS 走 CGEvent，Linux 走 XTest/libei）。
  - 剪贴板 fallback：备份原剪贴板 → 写入文本 → 模拟 Ctrl+V（用扫描码 `Key::Other(0x56)` 绕过键盘布局）→ 还原剪贴板。
  - 输入前会清理残留修饰键（等待热键修饰键释放 + 补发 key-up + 发 ESC 清菜单激活态），避免第一个字符被解释成菜单快捷键。
  - 当前只有 `windows_input.rs` 实现；macOS/Linux 走 `unsupported.rs`，主路径直接返回错误，需要补齐。
- `hotkey.rs`：`HotkeySpec` / `HotkeyKey`、`is_supported_hotkey` / `normalize_hotkey_for_current_platform` / `hotkey_pressed`（Windows 轮询用，其他平台暂为 stub）。
- `feedback.rs`：`FeedbackSound`（`Start` / `Stop` / `Complete` / `Error`）、`play_sound`。Windows 播放内嵌 WAV，其他平台 no-op。

新增平台能力时优先在这里加 `cfg` 子模块，保持上层接口稳定。

## apps/dou-voice-desktop

Tauri v2 shell。前端 + 后端 + build 脚本。

### 前端

- `main-src/App.jsx`：主窗口，包含 setup wizard（`Doubao Session` / `Input Basics` / `Press To Talk` / `Ready Check` 四步）、`General` / `Auth` / `Diagnostics` / `About` 四个分区。
- `overlay-src/RecordingOverlay.jsx`：overlay 浮层，监听 `voice-status` / `voice-text` / `mic-level` 事件，显示状态、文本和波形。
- `web/index.html` / `web/overlay.html`：Tauri 静态入口，由 `build-web.js` 把 React bundle 注入到 `web/scripts/main-react.js` 和 `web/scripts/overlay-react.js`。
- 前端通过 `window.__TAURI__.core.invoke` 调用 Rust 命令。

### src-tauri/src/

每个文件一个模块，职责单一（见 `main.rs` 的 `mod` 声明）：

- `main.rs`：注册 plugin、setup hook（初始化 auth 路径、设置、overlay、托盘、全局热键）、`invoke_handler` 注册全部 Tauri 命令、`on_window_event` 把主窗口关闭转为隐藏。
- `app_state.rs`：共享状态。
  - `DesktopState`：`auth_path` / `voice_busy` / `active_recording` / `voice_status` / `diagnostic_events` / `settings` / `user_settings_exists` / `hotkey`。CPAL stream 不放这里（Windows 上不是 `Send`），由 worker 线程持有。
  - `HotkeyRuntimeState`：`capture_active` / `pressed` / `suppressed_until_release` / `press_generation` / `last_press_at`，放在同一把锁下避免跨线程状态撕裂。
  - 常量：标签、文件名、默认值、超时（`HOTKEY_PRESS_DEBOUNCE=30ms`、`HOTKEY_RELEASE_FALLBACK_TIMEOUT=30s`、`OVERLAY_HIDE_DELAY=1.6s`、`MAX_DIAGNOSTIC_EVENTS=2000` 等）。
  - `LoginCaptureState`：登录窗口 localStorage 捕获态，用 `request_id` 避免读到上一轮残留。
- `voice.rs`：语音输入主链路。`begin_voice_input` / `finish_voice_input` / `record_once_and_type`（5 秒测试录音）/ `start_hotkey_recording_body` / `finish_hotkey_recording_body`。负责状态切换、提示音、文本输入、诊断事件。
- `voice_worker.rs`：`spawn_streaming_recording_worker`，在独立线程持有 CPAL stream，主线程只保存停止信号和结果接收端。
- `hotkey.rs`：全局热键生命周期。`setup_global_shortcut` 注册 Tauri global shortcut；`trigger_hotkey_pressed` / `trigger_hotkey_released` 处理按下/松开；Windows 在 `WINDOWS_HOTKEY_POLL_INTERVAL=30ms` 间隔轮询 `platform::hotkey_pressed`，因为部分 modifier-only 组合 Tauri 注册路径不支持。
- `macos_hotkey.rs`：macOS 专属热键辅助，`#[cfg(target_os = "macos")]` 且 `#[allow(unsafe_code)]`（workspace 其他地方 `unsafe_code = "forbid"`）。
- `auth_window.rs`：登录窗口和认证导出。
  - `open_login_window`：打开 `https://www.doubao.com/chat`，通过 `on_navigation` 拦截 `dou-voice.localhost/capture?...` 回传 localStorage。
  - `export_auth`：清空旧捕获 → 注入 JS 读取 `samantha_web_web_id` 和 `__tea_cache_tokens_497858` → 等待回传 → 从 WebView cookie store 读取 domain 含 `doubao.com` 的 Cookie（fallback 按域名查 `www.doubao.com`、`frontier-audio-web-ws.doubao.com`、`ws-samantha.doubao.com`）→ 校验后写入 `auth.json`。
- `settings.rs`：`get_settings` / `save_settings` / `get_available_input_devices` / `get_default_auth_path` / `initialize_settings` / `initialize_default_auth_path`。持久化 `AppSettings`（hotkey、inputMethod、selectedInputDevice、soundEnabled、overlayEnabled）。`settings.json` 不存在或损坏时回退默认值并显示 setup wizard。
- `diagnostics.rs`：`check_auth_status` / `export_diagnostics`。`DiagnosticsSnapshot` 包含 app 版本、平台、auth 摘要、ASR 摘要、当前 voice status、最近 2000 条事件。事件文本用预览，不写 Cookie / device_id / web_id 原文。
- `overlay.rs`：`setup_overlay` 创建 overlay 窗口，`update_overlay_status` 控制显隐和内容。Windows/Linux 用 Tauri 透明置顶 `WebviewWindowBuilder`；macOS 用 `tauri-nspanel` 创建 non-activating NSPanel（`can_become_key_window: false`、`is_floating_panel: true`）。默认 416×112，位于当前显示器工作区底部居中，底边距 56px。
- `tray.rs`：系统托盘图标、tooltip、菜单（`Show Window` / `Quit`）。双击托盘显示主窗口。图标随 voice status 变化。
- `window.rs`：`should_hide_on_close` / `show_main_window`。关闭主窗口和登录窗口转为隐藏，进程常驻托盘。
- `asr_options.rs`：`streaming_transcribe_options` 构造 ASR 流式参数。
- `build_info.rs`：读取 `build.rs` 写入的 git 元信息（commit hash、dirty 标记、构建时间、profile、target），暴露 `get_app_build_info` 命令。
- `util.rs`：`unix_time_ms` / `uuid_like_request_id` / `text_preview` 等小工具。

### src-tauri/build.rs

构建期做两件事：

1. 嵌入 git 元信息到编译期环境变量（`DOU_VOICE_COMMIT_HASH` 等）。
2. 自动构建前端 bundle：监听 `package.json` / `bun.lock` / `build-web.js` / `main-src` / `overlay-src` / `web/` 等路径，缺失 `node_modules` 时 `bun install`，然后 `bun run build:web`。`DOU_VOICE_SKIP_FRONTEND_BUILD=1` 可跳过。

## 关键链路

### ASR 链路

默认端点 `wss://frontier-audio-web-ws.doubao.com/api/v2/sami/voicegenie`，默认 Origin `https://www.doubao.com`。认证参数来自 `auth.json`。URL 参数模拟豆包 Web 端，包括 `api_app_key`、`namespace`、`aid=497858`、`device_id`、`web_id`、`web_tab_id` 等；`web_tab_id` 每次连接生成新 UUID。旧 `ws-samantha` 风格协议保留解析兼容，但默认路径是 VoiceGenie。

核心音频格式固定 `16kHz / mono / s16le PCM`，平台采集层在进入 ASR 前转换到该格式。

固定 5 秒测试入口：

```text
record_once_and_type(duration, device)
  → convert_to_16k_mono_s16le
  → transcribe_pcm_bytes
  → type_text_to_focused_window
```

press-to-talk 流式入口：

```text
hotkey pressed
  → load auth
  → spawn_streaming_recording_worker
  → CPAL callback emits PCM chunks
  → transcribe_pcm_stream_with_events
hotkey released
  → stop input stream
  → send tail silence
  → wait for final events
  → type recognized text
```

### 热键

默认 `Ctrl+Q`。Windows 用平台轮询支持 press-to-talk；macOS/Linux 用 `tauri-plugin-global-shortcut` 的 pressed/released 事件，当前只支持带主键的组合。

`HotkeyRuntimeState` 字段职责：

- `capture_active`：设置页捕获热键时暂停全局语音输入。
- `pressed`：避免重复 pressed。
- `suppressed_until_release`：忙碌时忽略本次按住直到松开。
- `press_generation`：为 release fallback 绑定具体按下事件，避免延迟 release 错配到下一次按下。
- `last_press_at`：30ms 防抖。

如果应用在录音、识别或输入时再次按下热键，会记录 `hotkey_ignored` 并提示用户等待上一段完成。30 秒 release fallback 避免极端情况下丢失 release 后一直录音。

### 文本输入

统一入口 `dou-voice-platform/src/input.rs`：

```text
type_text(text)              # 先 enigo 直接输入，失败回退剪贴板粘贴
copy_text_to_clipboard(text)  # 只写剪贴板，不自动粘贴
```

`Clipboard paste` 设置项调用 `copy_text_to_clipboard`，用户需要手动 `Ctrl+V`。

### 反馈和 UI

按重要性分层：

```text
核心状态：托盘 tooltip 和动态图标
录音反馈：系统提示音（Start / Stop / Complete / Error）
实时辅助：底部 overlay（状态、实时文本、麦克风波形）
排障信息：主窗口 Activity log 和 diagnostics JSON
```

### 诊断

诊断快照包含：生成时间、app version（含 git commit）、`std::env::consts::OS` 平台名、auth 摘要（路径、是否存在、cookie 数量、device/web id 是否存在）、ASR endpoint / origin / 流式参数、当前 voice status、最近 2000 条事件。事件文本用预览，不含 Cookie / device_id / web_id 原文。

## 当前边界

- Windows 是主验证路径，所有功能已真机验收。
- macOS：CI 通过编译和 bundle 构建；麦克风权限、辅助功能权限、登录 Cookie 提取、全局热键 pressed/released、输入模拟、overlay 焦点和多显示器位置需要真机验收；artifact 未做 Developer ID 签名和 notarization。
- Linux：CI 通过编译和 bundle 构建；X11 / Wayland / 不同桌面环境下的全局热键、输入模拟、overlay 层级需要分别真机验收。
- 豆包 Web 协议和 localStorage key（`samantha_web_web_id`、`__tea_cache_tokens_497858`）可能随服务端更新而变化，需要通过诊断和测试及时发现。
- `dou-voice-platform` 的 macOS/Linux `input`、`hotkey`、`feedback` 实现仍是 stub 或部分实现，需要按真实平台能力补齐。

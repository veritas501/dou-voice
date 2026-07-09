use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use dou_voice_core::PcmTranscribeOptions;
use serde::{Deserialize, Serialize};

pub(crate) const MAIN_LABEL: &str = "main";
pub(crate) const LOGIN_LABEL: &str = "doubao-login";
pub(crate) const OVERLAY_LABEL: &str = "voice-overlay";
pub(crate) const TRAY_ID: &str = "main";
pub(crate) const LOGIN_URL: &str = "https://www.doubao.com/chat";
pub(crate) const CAPTURE_HOST: &str = "dou-voice.localhost";
pub(crate) const CAPTURE_PATH: &str = "/capture";
pub(crate) const AUTH_FILE_NAME: &str = "auth.json";
pub(crate) const SETTINGS_FILE_NAME: &str = "settings.json";
pub(crate) const DEFAULT_RECORD_SECONDS: u64 = 5;
pub(crate) const DEFAULT_HOTKEY_LABEL: &str = "Ctrl+Q";
pub(crate) const INPUT_METHOD_DIRECT: &str = "direct";
pub(crate) const INPUT_METHOD_CLIPBOARD: &str = "clipboardPaste";
pub(crate) const OVERLAY_HIDE_DELAY: Duration = Duration::from_millis(1_600);
pub(crate) const OVERLAY_WIDTH: f64 = 416.0;
pub(crate) const OVERLAY_HEIGHT: f64 = 112.0;
pub(crate) const OVERLAY_BOTTOM_MARGIN_PX: i32 = 56;
pub(crate) const HOTKEY_RELEASE_FALLBACK_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const HOTKEY_PRESS_DEBOUNCE: Duration = Duration::from_millis(30);
#[cfg(windows)]
pub(crate) const WINDOWS_HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(30);
pub(crate) const TRAY_SHOW_ID: &str = "show_window";
pub(crate) const TRAY_QUIT_ID: &str = "quit";
pub(crate) const MAX_DIAGNOSTIC_EVENTS: usize = 2_000;

/// 登录窗口 localStorage 捕获状态。
///
/// Tauri 命令通过登录 WebView 注入 JS，再用本地占位 URL 回传 localStorage。这里仅保存
/// 最新一次捕获结果，并通过 request_id 避免读到上一轮残留数据。
#[derive(Debug, Default)]
pub(crate) struct LoginCaptureState {
    pub(crate) latest: Mutex<Option<StorageCapture>>,
}

/// 桌面应用运行态。
///
/// 该状态只保存 Tauri shell 需要跨命令共享的数据。CPAL stream 不直接放在这里，因为
/// Windows 上它不是 `Send`，实际录音对象由专门的 worker 线程持有。
pub(crate) struct DesktopState {
    pub(crate) auth_path: Mutex<PathBuf>,
    pub(crate) voice_busy: Mutex<bool>,
    pub(crate) active_recording: Mutex<Option<RecordingWorker>>,
    pub(crate) voice_status: Mutex<VoiceStatus>,
    pub(crate) diagnostic_events: Mutex<VecDeque<VoiceDebugEvent>>,
    pub(crate) settings: Mutex<AppSettings>,
    pub(crate) user_settings_exists: Mutex<bool>,
    pub(crate) hotkey: Mutex<HotkeyRuntimeState>,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            auth_path: Mutex::new(PathBuf::from(AUTH_FILE_NAME)),
            voice_busy: Mutex::new(false),
            active_recording: Mutex::new(None),
            voice_status: Mutex::new(VoiceStatus::idle()),
            diagnostic_events: Mutex::new(VecDeque::with_capacity(MAX_DIAGNOSTIC_EVENTS)),
            settings: Mutex::new(AppSettings::default()),
            user_settings_exists: Mutex::new(false),
            hotkey: Mutex::new(HotkeyRuntimeState::default()),
        }
    }
}

/// 全局热键运行态。
///
/// macOS 的 global shortcut 可能在按键重复或窗口切换时产生非常密集的事件；
/// 这里把 press/release、防抖和 busy 抑制状态放在同一把锁下，避免跨线程状态撕裂。
#[derive(Debug, Default)]
pub(crate) struct HotkeyRuntimeState {
    pub(crate) capture_active: bool,
    pub(crate) pressed: bool,
    pub(crate) suppressed_until_release: bool,
    pub(crate) press_generation: u64,
    pub(crate) last_press_at: Option<Instant>,
}

/// 桌面端可持久化设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSettings {
    #[serde(default = "default_hotkey_label")]
    pub(crate) hotkey: String,
    #[serde(default = "default_input_method")]
    pub(crate) input_method: String,
    #[serde(default)]
    pub(crate) selected_input_device: Option<String>,
    #[serde(default = "default_enabled")]
    pub(crate) sound_enabled: bool,
    #[serde(default = "default_enabled")]
    pub(crate) overlay_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey_label(),
            input_method: default_input_method(),
            selected_input_device: None,
            sound_enabled: true,
            overlay_enabled: true,
        }
    }
}

fn default_hotkey_label() -> String {
    DEFAULT_HOTKEY_LABEL.to_string()
}

fn default_input_method() -> String {
    INPUT_METHOD_DIRECT.to_string()
}

fn default_enabled() -> bool {
    true
}

/// 设置页渲染所需快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshot {
    pub(crate) settings: AppSettings,
    pub(crate) auth: AuthStatusResult,
    pub(crate) onboarding_required: bool,
}

/// 设置页展示的录音输入设备。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AudioInputDeviceResult {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) is_default: bool,
}

/// 从豆包页面 localStorage 中捕获到的最小认证字段。
///
/// Cookie 通过 WebView cookie API 读取；localStorage 只负责提供 `device_id` 和 `web_id`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageCapture {
    pub(crate) request_id: String,
    pub(crate) device_id: String,
    pub(crate) web_id: String,
}

/// 导出 auth.json 后返回给前端的摘要。
///
/// 不返回 Cookie 原文，避免 UI 日志泄漏敏感值。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportAuthResult {
    pub(crate) output_path: String,
    pub(crate) cookie_count: usize,
    pub(crate) device_id_present: bool,
    pub(crate) web_id_present: bool,
}

/// 导出诊断文件后的摘要。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportDiagnosticsResult {
    pub(crate) output_path: String,
    pub(crate) event_count: usize,
}

/// 认证状态摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthStatusResult {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) load_ok: bool,
    pub(crate) cookie_count: Option<usize>,
    pub(crate) device_id_present: Option<bool>,
    pub(crate) web_id_present: Option<bool>,
    pub(crate) captured_at_unix_ms: Option<u64>,
    pub(crate) error: Option<String>,
}

/// 写入诊断 JSON 的脱敏快照。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticsSnapshot {
    pub(crate) generated_at_unix_ms: u64,
    pub(crate) app_version: &'static str,
    pub(crate) build: AppBuildInfo,
    pub(crate) platform: &'static str,
    pub(crate) auth: DiagnosticsAuthSummary,
    pub(crate) asr: DiagnosticsAsrSummary,
    pub(crate) voice_status: VoiceStatus,
    pub(crate) events: Vec<VoiceDebugEvent>,
}

/// 编译进二进制的版本与构建来源信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppBuildInfo {
    pub(crate) version: String,
    pub(crate) commit_hash: String,
    pub(crate) commit_short_hash: String,
    pub(crate) git_dirty: bool,
    pub(crate) build_unix_ms: u64,
    pub(crate) profile: String,
    pub(crate) target: String,
}

/// 认证文件脱敏摘要。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticsAuthSummary {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) load_ok: bool,
    pub(crate) cookie_count: Option<usize>,
    pub(crate) device_id_present: Option<bool>,
    pub(crate) web_id_present: Option<bool>,
    pub(crate) captured_at_unix_ms: Option<u64>,
    pub(crate) error: Option<String>,
}

/// ASR 配置脱敏摘要。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticsAsrSummary {
    pub(crate) endpoint: String,
    pub(crate) origin: String,
    pub(crate) streaming_options: PcmTranscribeOptionsSummary,
}

/// 实时识别参数摘要。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PcmTranscribeOptionsSummary {
    pub(crate) chunk_bytes: usize,
    pub(crate) chunk_delay_ms: u64,
    pub(crate) tail_silence_ms: u64,
    pub(crate) receive_timeout_ms: u64,
    pub(crate) post_input_receive_timeout_ms: u64,
}

impl From<PcmTranscribeOptions> for PcmTranscribeOptionsSummary {
    fn from(options: PcmTranscribeOptions) -> Self {
        Self {
            chunk_bytes: options.chunk_bytes,
            chunk_delay_ms: options.chunk_delay_ms,
            tail_silence_ms: options.tail_silence_ms,
            receive_timeout_ms: options.receive_timeout_ms,
            post_input_receive_timeout_ms: options.post_input_receive_timeout_ms,
        }
    }
}

impl From<AuthStatusResult> for DiagnosticsAuthSummary {
    fn from(auth: AuthStatusResult) -> Self {
        Self {
            path: auth.path,
            exists: auth.exists,
            load_ok: auth.load_ok,
            cookie_count: auth.cookie_count,
            device_id_present: auth.device_id_present,
            web_id_present: auth.web_id_present,
            captured_at_unix_ms: auth.captured_at_unix_ms,
            error: auth.error,
        }
    }
}

/// 主窗口展示的语音输入状态。
///
/// `phase` 保持稳定英文值，前端可直接用它做样式映射；`message` 是用户可见英文说明。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceStatus {
    pub(crate) phase: String,
    pub(crate) message: String,
    pub(crate) last_text: Option<String>,
}

impl VoiceStatus {
    /// 创建初始空闲状态。
    pub(crate) fn idle() -> Self {
        Self {
            phase: "idle".to_string(),
            message: "Ready.".to_string(),
            last_text: None,
        }
    }
}

/// 发送到前端 Activity 面板的调试事件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceDebugEvent {
    pub(crate) timestamp_unix_ms: u64,
    pub(crate) stage: String,
    pub(crate) message: String,
    pub(crate) chunk_count: Option<usize>,
    pub(crate) pcm_bytes: Option<usize>,
    pub(crate) text: Option<String>,
}

/// 一次语音输入完成后的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceInputResult {
    pub(crate) final_text: String,
    pub(crate) pcm_bytes: usize,
}

pub(crate) struct StreamingRecognitionResult {
    pub(crate) events: Vec<dou_voice_core::AsrEvent>,
    pub(crate) pcm_bytes: usize,
}

#[derive(Default)]
pub(crate) struct AsrDebugLogState {
    pub(crate) last_partial_text: Option<String>,
    pub(crate) repeated_partial_count: usize,
}

pub(crate) struct RecordingWorker {
    pub(crate) stop_tx: mpsc::Sender<()>,
    pub(crate) result_rx: mpsc::Receiver<Result<StreamingRecognitionResult, String>>,
}

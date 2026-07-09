use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dou_voice_core::{AuthParams, AuthParamsStore};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

use crate::app_state::{
    DesktopState, ExportAuthResult, LoginCaptureState, StorageCapture, CAPTURE_HOST, CAPTURE_PATH,
    LOGIN_LABEL, LOGIN_URL,
};
use crate::util::{unix_time_ms, uuid_like_request_id};

/// 打开或聚焦豆包登录窗口。
///
/// 登录窗口只用于提取认证参数；识别阶段不依赖网页 UI。
#[tauri::command]
pub(crate) async fn open_login_window(app: AppHandle<Wry>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LOGIN_LABEL) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let url = LOGIN_URL
        .parse()
        .map(WebviewUrl::External)
        .map_err(|error| format!("invalid login url: {error}"))?;

    WebviewWindowBuilder::new(&app, LOGIN_LABEL, url)
        .title("Doubao Login")
        .inner_size(1100.0, 760.0)
        .resizable(true)
        .on_navigation({
            let app = app.clone();
            move |url| {
                // 通过拦截本地占位 URL 接收页面 localStorage，避免依赖远端页面加载 Tauri API。
                if url.host_str() == Some(CAPTURE_HOST) && url.path() == CAPTURE_PATH {
                    capture_storage_from_url(&app, url);
                    return false;
                }
                true
            }
        })
        .build()
        .map_err(|error| error.to_string())?;

    Ok(())
}

/// 从登录窗口导出认证参数到指定路径。
///
/// 流程是：清空旧 localStorage 捕获 -> 注入读取脚本 -> 等待回传 -> 读取 Cookie ->
/// 校验并写入 auth.json。成功后同步更新桌面端当前 auth 路径。
#[tauri::command]
pub(crate) async fn export_auth(
    app: AppHandle<Wry>,
    state: tauri::State<'_, LoginCaptureState>,
    output_path: String,
) -> Result<ExportAuthResult, String> {
    let login = app
        .get_webview_window(LOGIN_LABEL)
        .ok_or_else(|| "Open the login window and sign in first.".to_string())?;

    let request_id = uuid_like_request_id();
    {
        let mut latest = state
            .latest
            .lock()
            .map_err(|_| "localStorage capture state poisoned".to_string())?;
        *latest = None;
    }
    login
        .eval(local_storage_capture_script(&request_id))
        .map_err(|error| format!("failed to read localStorage: {error}"))?;

    let storage = wait_for_storage_capture(&state, &request_id)?;
    let cookies = read_doubao_cookies(&login)?;
    let params = AuthParams {
        cookies,
        device_id: storage.device_id,
        web_id: storage.web_id,
        captured_at_unix_ms: unix_time_ms(),
    };
    params.validate().map_err(|error| error.to_string())?;

    let output_path = PathBuf::from(output_path);
    let store = AuthParamsStore::new(&output_path);
    store.save(&params).map_err(|error| error.to_string())?;
    {
        let desktop_state = app.state::<DesktopState>();
        let mut auth_path = desktop_state
            .auth_path
            .lock()
            .map_err(|_| "desktop auth path state poisoned".to_string())?;
        *auth_path = output_path;
    }

    Ok(ExportAuthResult {
        output_path: store.path().display().to_string(),
        cookie_count: params.cookies.len(),
        device_id_present: !params.device_id.is_empty(),
        web_id_present: !params.web_id.is_empty(),
    })
}

/// 读取豆包 Web 和 ASR 域名下的 Cookie。
///
/// ASR 握手依赖 `.doubao.com` 域下的 HTTP-only Cookie。优先读取 WebView runtime
/// 的全量 cookie store，并按 domain 过滤；这和 WKWebView 原生实现一致。若平台返回为空，
/// 再退回到按 URL 查询的路径匹配方式。
fn read_doubao_cookies(
    window: &tauri::WebviewWindow<Wry>,
) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();

    for cookie in window
        .cookies()
        .map_err(|error| format!("failed to read cookies: {error}"))?
    {
        if cookie
            .domain()
            .is_some_and(|domain| domain.contains("doubao.com"))
        {
            values.insert(cookie.name().to_string(), cookie.value().to_string());
        }
    }

    if !values.is_empty() {
        return Ok(values);
    }

    for url in [
        "https://www.doubao.com",
        "https://frontier-audio-web-ws.doubao.com",
        "https://ws-samantha.doubao.com",
    ] {
        let url = tauri::Url::parse(url).map_err(|error| error.to_string())?;
        let cookies = window
            .cookies_for_url(url)
            .map_err(|error| format!("failed to read cookies: {error}"))?;
        for cookie in cookies {
            values.insert(cookie.name().to_string(), cookie.value().to_string());
        }
    }
    Ok(values)
}

/// 等待登录窗口脚本回传 localStorage。
///
/// 使用短超时是为了让前端快速得到可操作错误；如果用户尚未登录，应该重新打开登录窗确认。
fn wait_for_storage_capture(
    state: &tauri::State<'_, LoginCaptureState>,
    request_id: &str,
) -> Result<StorageCapture, String> {
    let started_at = Instant::now();
    while started_at.elapsed() < Duration::from_secs(3) {
        if let Some(capture) = state
            .latest
            .lock()
            .map_err(|_| "localStorage capture state poisoned".to_string())?
            .clone()
        {
            if capture.request_id == request_id {
                return Ok(capture);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("did not receive localStorage capture; confirm the login window is signed in and still open".to_string())
}

/// 生成注入到豆包登录页的 localStorage 读取脚本。
///
/// 脚本只读取当前需要的字段，并跳转到本地占位 URL 让 Tauri navigation hook 捕获。
fn local_storage_capture_script(request_id: &str) -> String {
    format!(
        r#"
(() => {{
  const requestId = {request_id_json};
  const readJson = (key) => {{
    try {{
      const value = window.localStorage.getItem(key);
      return value ? JSON.parse(value) : null;
    }} catch (_) {{
      return null;
    }}
  }};
  const device = readJson("samantha_web_web_id");
  const tea = readJson("__tea_cache_tokens_497858");
  const payload = {{
    requestId,
    deviceId: (device && (device.web_id || device.webId || device.id)) || "",
    webId: (tea && (tea.web_id || tea.webId || tea.user_unique_id)) || ""
  }};
  const encoded = encodeURIComponent(JSON.stringify(payload));
  window.location.href = "https://{capture_host}{capture_path}?payload=" + encoded;
}})();
"#,
        request_id_json = serde_json::to_string(request_id).expect("serialize request id"),
        capture_host = CAPTURE_HOST,
        capture_path = CAPTURE_PATH
    )
}

/// 处理登录页跳转到本地占位 URL 时携带的捕获结果。
///
/// 该函数只在 payload 可解析时更新状态；无效导航保持静默，避免远端页面异常影响登录窗。
fn capture_storage_from_url(app: &AppHandle<Wry>, url: &tauri::Url) {
    let Some((_, payload)) = url.query_pairs().find(|(name, _)| name == "payload") else {
        return;
    };
    let Ok(capture) = serde_json::from_str::<StorageCapture>(&payload) else {
        return;
    };
    let state = app.state::<LoginCaptureState>();
    {
        if let Ok(mut latest) = state.latest.lock() {
            *latest = Some(capture);
        };
    }
}

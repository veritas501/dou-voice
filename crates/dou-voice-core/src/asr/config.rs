use crate::{AuthParams, CoreError, CoreResult};

pub(crate) const VOICEGENIE_APP_KEY: &str = "GOqQpfo1fO7slHv8";
pub(crate) const VOICEGENIE_NAMESPACE: &str = "VoiceGenie";
pub(crate) const VOICEGENIE_START_TASK: &str = "StartTask";
pub(crate) const VOICEGENIE_TASK_STARTED: &str = "TaskStarted";
pub(crate) const VOICEGENIE_START_SESSION: &str = "StartSession";
pub(crate) const VOICEGENIE_SESSION_STARTED: &str = "SessionStarted";
pub(crate) const VOICEGENIE_TASK_REQUEST: &str = "TaskRequest";
pub(crate) const VOICEGENIE_END_ASR: &str = "EndASR";
pub(crate) const VOICEGENIE_ASR_ENDED: &str = "ASREnded";
pub(crate) const VOICEGENIE_FINISH_SESSION: &str = "FinishSession";
pub(crate) const VOICEGENIE_SESSION_FAILED: &str = "SessionFailed";
pub(crate) const VOICEGENIE_TASK_FAILED: &str = "TaskFailed";
pub(crate) const VOICEGENIE_ASR_RESPONSE: &str = "ASRResponse";

/// 豆包 ASR WebSocket 客户端配置。
///
/// `endpoint` 指向 ASR WebSocket 服务，`origin` 会作为请求头发送以模拟网页版调用上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrClientConfig {
    /// ASR WebSocket 端点。
    pub endpoint: String,
    /// WebSocket 握手时使用的 Origin。
    pub origin: String,
}

impl Default for AsrClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "wss://frontier-audio-web-ws.doubao.com/api/v2/sami/voicegenie".to_string(),
            origin: "https://www.doubao.com".to_string(),
        }
    }
}

impl AsrClientConfig {
    /// 使用认证参数和当前会话的 `web_tab_id` 构造 ASR WebSocket URL。
    ///
    /// URL 参数由认证参数和会话上下文组成。调用方必须保证 `auth` 来自有效登录态。
    pub fn build_url(&self, auth: &AuthParams, web_tab_id: &str) -> CoreResult<String> {
        auth.validate()?;
        if web_tab_id.trim().is_empty() {
            return Err(CoreError::InvalidAuth("web_tab_id is empty".to_string()));
        }

        let params = if self.wire_protocol() == AsrWireProtocol::VoiceGenie {
            self.voicegenie_params(auth, web_tab_id)
        } else {
            self.legacy_samantha_params(auth, web_tab_id)
        };

        let query = params
            .iter()
            .map(|(name, value)| format!("{name}={}", encode_query_value(value)))
            .collect::<Vec<_>>()
            .join("&");

        Ok(format!("{}?{}", self.endpoint, query))
    }

    pub(crate) fn wire_protocol(&self) -> AsrWireProtocol {
        if self.endpoint.contains("/api/v2/sami/voicegenie") {
            AsrWireProtocol::VoiceGenie
        } else {
            AsrWireProtocol::LegacyRawPcm
        }
    }

    fn voicegenie_params(
        &self,
        auth: &AuthParams,
        web_tab_id: &str,
    ) -> Vec<(&'static str, String)> {
        vec![
            ("api_app_key", VOICEGENIE_APP_KEY.to_string()),
            ("namespace", VOICEGENIE_NAMESPACE.to_string()),
            ("version_code", "20800".to_string()),
            ("language", "zh".to_string()),
            ("device_platform", "web".to_string()),
            ("pkg_type", "release_version".to_string()),
            ("pc_version", "3.26.0".to_string()),
            ("region", "CN".to_string()),
            ("sys_region", "CN".to_string()),
            ("samantha_web", "1".to_string()),
            ("use-olympus-account", "1".to_string()),
            ("aid", "497858".to_string()),
            ("real_aid", "497858".to_string()),
            ("device_id", auth.device_id.clone()),
            ("web_id", auth.web_id.clone()),
            ("tea_uuid", auth.web_id.clone()),
            ("web_platform", "browser".to_string()),
            ("web_tab_id", web_tab_id.to_string()),
        ]
    }

    fn legacy_samantha_params(
        &self,
        auth: &AuthParams,
        web_tab_id: &str,
    ) -> Vec<(&'static str, String)> {
        vec![
            ("version_code", "20800"),
            ("language", "zh"),
            ("device_platform", "web"),
            ("aid", "497858"),
            ("real_aid", "497858"),
            ("pkg_type", "release_version"),
            ("device_id", auth.device_id.as_str()),
            ("pc_version", "3.12.3"),
            ("web_id", auth.web_id.as_str()),
            ("tea_uuid", auth.web_id.as_str()),
            ("region", ""),
            ("sys_region", ""),
            ("samantha_web", "1"),
            ("use-olympus-account", "1"),
            ("web_tab_id", web_tab_id),
            ("format", "pcm"),
        ]
        .into_iter()
        .map(|(name, value)| (name, value.to_string()))
        .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsrWireProtocol {
    LegacyRawPcm,
    VoiceGenie,
}

/// 仅覆盖当前 URL 参数所需的最小 percent-encoding。
pub(crate) fn encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

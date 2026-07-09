use std::fmt::{Display, Formatter};

/// 核心库统一使用的结果类型。
pub type CoreResult<T> = Result<T, CoreError>;

/// 核心链路可返回的稳定错误分类。
///
/// 这里保留面向上层 UI 的英文错误文本，但枚举本身只表达错误类型。平台层如果
/// 需要更友好的提示，应在边界处映射，不要把平台文案写回核心 crate。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// 文件读写或目录创建失败。
    Io(String),
    /// JSON 序列化或解析失败。
    Json(String),
    /// 状态机收到当前状态不允许处理的命令。
    InvalidState {
        /// 当前状态的稳定英文名。
        current: &'static str,
        /// 触发命令的稳定英文名。
        command: &'static str,
    },
    /// 未提供认证参数文件或认证参数为空。
    MissingAuth,
    /// 认证参数结构存在，但缺少 ASR 连接所需字段。
    InvalidAuth(String),
    /// 输入设备、音频格式或 PCM 数据不符合要求。
    AudioUnavailable(String),
    /// WebSocket 连接、发送、接收或协议解析失败。
    AsrConnection(String),
    /// 豆包服务端明确返回认证过期或无效。
    AuthExpired,
}

impl Display for CoreError {
    /// 输出稳定英文错误消息，供桌面日志和 UI 状态直接展示。
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "io error: {message}"),
            Self::Json(message) => write!(f, "json error: {message}"),
            Self::InvalidState { current, command } => {
                write!(
                    f,
                    "invalid state transition: {current} cannot handle {command}"
                )
            }
            Self::MissingAuth => write!(f, "missing auth parameters"),
            Self::InvalidAuth(message) => write!(f, "invalid auth parameters: {message}"),
            Self::AudioUnavailable(message) => write!(f, "audio unavailable: {message}"),
            Self::AsrConnection(message) => write!(f, "asr connection error: {message}"),
            Self::AuthExpired => write!(f, "auth parameters expired"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<std::io::Error> for CoreError {
    /// 保留底层 IO 错误文本，避免在核心层丢失具体路径或权限信息。
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    /// 保留 serde_json 的结构化解析提示，便于用户定位损坏的 auth.json。
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

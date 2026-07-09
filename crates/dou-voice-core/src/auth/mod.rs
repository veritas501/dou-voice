use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult};

mod json;

/// 豆包 Web 登录态中提取出的 ASR 认证参数。
///
/// Cookie、`device_id` 和 `web_id` 都是连接 ASR WebSocket 的必要参数。该结构会被
/// 序列化到本地 `auth.json`，因此不得写入仓库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthParams {
    /// 从豆包和 ASR 域名读取到的 Cookie map。
    pub cookies: BTreeMap<String, String>,
    /// 豆包 Web 端设备标识。
    pub device_id: String,
    /// 豆包 Web 端用户/浏览器标识。
    pub web_id: String,
    /// 提取时间戳，单位毫秒。
    #[serde(default)]
    pub captured_at_unix_ms: u64,
}

impl AuthParams {
    /// 判断是否包含 ASR 连接所需的最小字段。
    pub fn is_complete(&self) -> bool {
        !self.cookies.is_empty() && !self.device_id.is_empty() && !self.web_id.is_empty()
    }

    /// 校验认证参数是否可用于 ASR 连接。
    pub fn validate(&self) -> CoreResult<()> {
        if self.cookies.is_empty() {
            return Err(CoreError::InvalidAuth("cookies is empty".to_string()));
        }
        if self.device_id.trim().is_empty() {
            return Err(CoreError::InvalidAuth("device_id is empty".to_string()));
        }
        if self.web_id.trim().is_empty() {
            return Err(CoreError::InvalidAuth("web_id is empty".to_string()));
        }
        Ok(())
    }

    /// 构造 WebSocket `Cookie` 请求头。
    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// 本地认证参数存储。
///
/// 路径由调用方决定：桌面端默认使用 Tauri app config 目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthParamsStore {
    path: PathBuf,
}

impl AuthParamsStore {
    /// 创建指向指定路径的认证参数 store。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 返回当前 store 路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 判断认证文件是否存在。
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// 读取、解析并校验认证参数。
    pub fn load(&self) -> CoreResult<AuthParams> {
        let data = fs::read_to_string(&self.path)?;
        let params = json::parse_auth_params(&data)?;
        params.validate()?;
        Ok(params)
    }

    /// 校验并保存认证参数。
    pub fn save(&self, params: &AuthParams) -> CoreResult<()> {
        params.validate()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = json::to_pretty_json(params)?;
        fs::write(&self.path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthParams, AuthParamsStore};
    use std::collections::BTreeMap;
    use std::fs;

    fn complete_params() -> AuthParams {
        AuthParams {
            cookies: BTreeMap::from([("session".to_string(), "value".to_string())]),
            device_id: "device".to_string(),
            web_id: "web".to_string(),
            captured_at_unix_ms: 1,
        }
    }

    #[test]
    fn complete_auth_requires_cookie_and_ids() {
        let params = complete_params();

        assert!(params.is_complete());
    }

    #[test]
    fn builds_cookie_header() {
        let params = AuthParams {
            cookies: BTreeMap::from([
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
            ]),
            device_id: "device".to_string(),
            web_id: "web".to_string(),
            captured_at_unix_ms: 1,
        };

        assert_eq!(params.cookie_header(), "a=1; b=2");
    }

    #[test]
    fn store_round_trips_params() {
        let path = std::env::temp_dir().join(format!(
            "dou-voice-auth-{}-{}.json",
            std::process::id(),
            "round-trip"
        ));
        let store = AuthParamsStore::new(&path);
        let params = complete_params();

        store.save(&params).expect("save params");
        let loaded = store.load().expect("load params");
        let _ = fs::remove_file(path);

        assert_eq!(loaded, params);
    }

    #[test]
    fn rejects_empty_cookie_map() {
        let params = AuthParams {
            cookies: BTreeMap::new(),
            device_id: "device".to_string(),
            web_id: "web".to_string(),
            captured_at_unix_ms: 1,
        };

        assert!(params.validate().is_err());
    }
}

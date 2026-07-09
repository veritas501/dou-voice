use crate::{AuthParams, CoreResult};

/// 将认证参数序列化为便于人工诊断的 pretty JSON。
///
/// 输出文件包含 Cookie 和设备标识，调用方必须保证路径位于用户本地配置目录或
/// 显式传入的安全位置，不得写入仓库。
pub fn to_pretty_json(params: &AuthParams) -> CoreResult<String> {
    Ok(serde_json::to_string_pretty(params)?)
}

/// 从 auth.json 文本解析认证参数。
///
/// 该函数只负责 JSON 反序列化；字段完整性由 `AuthParams::validate` 在 store 边界统一校验。
pub fn parse_auth_params(input: &str) -> CoreResult<AuthParams> {
    Ok(serde_json::from_str(input)?)
}

#[cfg(test)]
mod tests {
    use super::{parse_auth_params, to_pretty_json};
    use crate::AuthParams;
    use std::collections::BTreeMap;

    #[test]
    fn parses_auth_json() {
        let params = parse_auth_params(
            r#"{
              "cookies": {"sessionid": "abc", "sid_tt": "def"},
              "device_id": "device",
              "web_id": "web",
              "captured_at_unix_ms": 42
            }"#,
        )
        .expect("parse params");

        assert_eq!(params.cookies["sessionid"], "abc");
        assert_eq!(params.device_id, "device");
        assert_eq!(params.web_id, "web");
        assert_eq!(params.captured_at_unix_ms, 42);
    }

    #[test]
    fn pretty_json_round_trips_escaped_strings() {
        let params = AuthParams {
            cookies: BTreeMap::from([("sid\"tt".to_string(), "a\\b".to_string())]),
            device_id: "device\nid".to_string(),
            web_id: "web".to_string(),
            captured_at_unix_ms: 42,
        };

        let json = to_pretty_json(&params).expect("serialize params");
        let parsed = parse_auth_params(&json).expect("parse generated json");

        assert_eq!(parsed, params);
    }
}

use crate::app_state::AppBuildInfo;

/// 当前二进制携带的构建信息。
pub(crate) fn app_build_info() -> AppBuildInfo {
    AppBuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit_hash: option_env!("DOU_VOICE_COMMIT_HASH")
            .unwrap_or("unknown")
            .to_string(),
        commit_short_hash: option_env!("DOU_VOICE_COMMIT_SHORT_HASH")
            .unwrap_or("unknown")
            .to_string(),
        git_dirty: parse_bool(option_env!("DOU_VOICE_GIT_DIRTY").unwrap_or("false")),
        build_unix_ms: option_env!("DOU_VOICE_BUILD_UNIX_MS")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        profile: option_env!("DOU_VOICE_BUILD_PROFILE")
            .unwrap_or("unknown")
            .to_string(),
        target: option_env!("DOU_VOICE_BUILD_TARGET")
            .unwrap_or("unknown")
            .to_string(),
    }
}

#[tauri::command]
pub(crate) fn get_app_build_info() -> AppBuildInfo {
    app_build_info()
}

fn parse_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "True")
}

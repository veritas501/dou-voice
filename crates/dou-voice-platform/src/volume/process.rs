//! 音量 duck 用的外部命令辅助：统一收集 exit code / stderr，生成可诊断错误文案。

use std::process::Command;

/// 运行外部命令，成功时返回 stdout（trim 后）。
///
/// 失败时错误字符串尽量直白：程序不存在、非零退出、stderr 摘要。
pub(super) fn run_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("`{program}` not found on PATH")
        } else {
            format!("failed to start `{program}`: {error}")
        }
    })?;

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let stderr = summarize_bytes(&output.stderr);
        let stdout = summarize_bytes(&output.stdout);
        return Err(match (stderr.is_empty(), stdout.is_empty()) {
            (false, _) => format!("`{program}` exited with status {code}: {stderr}"),
            (true, false) => format!("`{program}` exited with status {code}: {stdout}"),
            (true, true) => format!("`{program}` exited with status {code}"),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn summarize_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 240;
    if collapsed.chars().count() <= MAX {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(MAX).collect();
    format!("{truncated}...")
}

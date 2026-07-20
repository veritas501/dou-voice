## 语言规范

你可以用英文思考，但请使用中文回答用户（除非用户特殊要求）

## 代码规范

- 写代码时需要添加适量的注释和文档增加代码的可读性和可维护性
- 注释、文档可以用中文，但报错、字符串必须用英文
- 代码模块清晰，一个文件不要超过500行，多了就要分模块了

## 提交前检查（必须）

在 `git commit` / `git push` 之前，本地至少跑通与 CI 等价的检查，避免把 format/clippy 错误推到远端：

1. **Format**：`cargo fmt --all`
2. **Clippy**（与 CI 一致，把警告当错误）：
   - Windows 原生：`cargo.exe clippy --workspace --all-targets -- -D warnings`
   - 或完整 CI：`bun scripts/ci.js`（含前端构建、fmt check、check、test、clippy）
3. 改动涉及平台热键/输入等 Windows 专用路径时，用 **Windows 原生** `cargo.exe` + `x86_64-pc-windows-msvc` 验证，不要只靠 WSL Linux target。

CI 入口：`scripts/ci.js`（`cargo fmt --check` → `cargo check` → `cargo test` → `cargo clippy -D warnings`）。

## Git hooks（推荐）

仓库提供可共享的 hooks 目录 `.githooks/`。克隆或拉取后执行一次：

```bash
git config core.hooksPath .githooks
```

`pre-commit` 会对 **已暂存的 Rust 变更** 跑 `cargo fmt --all` 与  
`cargo clippy --workspace --all-targets -- -D warnings`（设置 `DOU_VOICE_SKIP_FRONTEND_BUILD=1` 跳过前端重建）。失败则阻止提交。

未配置 hooksPath 时，仍须按上面的「提交前检查」手工执行。

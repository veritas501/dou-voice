# 开发与验证

本文档面向「想在这个仓库里改代码、跑测试、出包」的开发者。使用方式和初次上手见根 [README](../README.md)，代码组织见 [架构说明](architecture.md)，Windows 平台验收和排障见 [Windows 使用与验收](windows-desktop.md)。

## 仓库结构

```text
apps/dou-voice-desktop/
  src-tauri/          Tauri Rust 后端：命令、托盘、热键、overlay、录音 worker
    src/              每个文件一个模块，参考 architecture.md
    build.rs          构建期嵌入 git 元信息并自动构建前端 bundle
    tauri.conf.json   应用标识、窗口、bundle 目标
    capabilities/     Tauri ACL capability 定义
  main-src/           React 主窗口源码（setup wizard、设置、诊断）
  overlay-src/        React overlay 源码（状态浮层）
  web/                前端静态资源和构建输出（index.html、overlay.html、styles、scripts）
  build-web.js        Bun 打包脚本，由 build.rs 调用
  package.json        前端依赖，packageManager=bun@1.2.20
crates/
  dou-voice-core/     跨平台核心：ASR 协议、音频采集、认证存储、状态机、错误模型
  dou-voice-platform/ 平台适配：文本输入、热键解析、反馈音
docs/                 项目文档
scripts/
  ci.js               本地 CI 入口（bun install + 前端构建 + Rust 检查）
  generate_feedback_sounds.py  重新生成内嵌提示音 WAV
```

Cargo workspace 包含三个成员：`apps/dou-voice-desktop/src-tauri`、`crates/dou-voice-core`、`crates/dou-voice-platform`。Workspace 锁定 `rust-version = "1.78"`，并启用 `unsafe_code = "forbid"`、`dbg_macro/todo/unwrap_used = "deny"` 的全局 lint。

## 前置环境

通用：

- **Rust stable**（≥ 1.78，工作区已锁定）。
- **Bun**（`package.json` 指定 `bun@1.2.20`，CI 使用 latest）。
- 可用麦克风。
- 可登录豆包 Web。

Windows：

- 不要在 WSL2 中验证 Windows 桌面行为。Tauri 桌面构建路径与 Linux 不同，且输入模拟、托盘、overlay 行为只在 Windows 原生 toolchain 下被验证。
- WebView2 Runtime（Windows 11 通常已内置；Windows 10 可能需要手动安装）。

Linux（构建 Tauri 需要的系统库，与 CI 一致）：

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev libx11-dev libxtst-dev libxrandr-dev
```

macOS：

- 目前 macOS 构建只在 CI 上跑通。真机使用还需要授予麦克风权限、辅助功能权限，并验证全局热键 pressed/released、文本输入模拟、overlay 焦点行为。

## 常用命令

### 一键本地检查

```powershell
bun run scripts/ci.js
```

`scripts/ci.js` 会执行：`bun install` → `bun run build:web` → `cargo fmt --check` → `cargo check --workspace` → `cargo test --workspace` → `cargo clippy --workspace --all-targets -- -D warnings`。任一步失败即终止。CI 上跑的就是同一条命令。

### 跳过前端构建

前端 bundle 已经构建过、且只改 Rust 代码时：

```powershell
bun run scripts/ci.js --skip-frontend-install
```

它会设置 `DOU_VOICE_SKIP_FRONTEND_BUILD=1` 让 `src-tauri/build.rs` 跳过 `bun install` 和 `build:web`。注意：`web/scripts/main-react.js` 和 `web/scripts/overlay-react.js` 必须已经是当前源码的产物，否则运行的是过期前端。

直接走 cargo 时也可以手动设置该环境变量：

```powershell
$env:DOU_VOICE_SKIP_FRONTEND_BUILD = "1"
cargo check --workspace
```

### 单项命令

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

### 运行桌面应用

```powershell
cargo run -p dou-voice-desktop
```

首次运行（没有 `settings.json`）会进入 setup wizard。关闭主窗口不会退出进程，需要从托盘菜单 `Quit`。

### 手动构建前端

通常不需要，`build.rs` 会处理。需要单独跑：

```powershell
cd apps\dou-voice-desktop
bun run build:web       # 同时构建主窗口和 overlay
bun run build:overlay   # 仅 overlay
```

## 前端与 Cargo build 的关系

`apps/dou-voice-desktop/src-tauri/build.rs` 在每次 cargo build 时：

1. 通过 `CARGO_MANIFEST_DIR` 定位 desktop 目录，监听 `package.json`、`bun.lock`、`build-web.js`、`main-src`、`overlay-src`、`web/index.html`、`web/overlay.html`、`web/scripts/tauri-api.js`、`web/styles` 等路径。
2. 通过 git 命令把 `DOU_VOICE_COMMIT_HASH`、`DOU_VOICE_COMMIT_SHORT_HASH`、`DOU_VOICE_GIT_DIRTY`、`DOU_VOICE_BUILD_UNIX_MS`、`DOU_VOICE_BUILD_PROFILE`、`DOU_VOICE_BUILD_TARGET` 写进编译期环境变量，由 `build_info.rs` 暴露给前端。
3. 若 `DOU_VOICE_SKIP_FRONTEND_BUILD` 未设置：检查 `node_modules`，缺失则 `bun install`；然后 `bun run build:web`。
4. 调用 `tauri_build::build()` 完成 Tauri 资源收集。

不要在 release 或打包前跳过前端构建，除非已确认前端 bundle 是当前源码产物。

## 打包

打包需要 `cargo tauri` 子命令。安装：

```powershell
cargo install tauri-cli --locked
```

在本目录运行（Tauri CLI 会从 `tauri.conf.json` 读配置）：

```powershell
cd apps\dou-voice-desktop
cargo tauri build
```

或指定 bundle 类型：

```powershell
cargo tauri build --bundles nsis         # Windows
cargo tauri build --bundles app,dmg      # macOS
cargo tauri build --bundles deb,appimage # Linux
```

应用元数据（来自 `tauri.conf.json`）：

- `productName = "DouVoice"`
- 窗口标题 = `"Dou Voice"`
- `identifier = "dou.voice"`
- Windows NSIS `installMode = "currentUser"`

Windows NSIS 输出路径通常是：

```text
apps\dou-voice-desktop\src-tauri\target\release\bundle\nsis\
```

> 平台能力（输入模拟、热键、overlay 层级）请按真机验收后再分发，详见 [Windows 使用与验收](windows-desktop.md) 和下文「平台验证边界」。macOS artifact 当前未配置 Developer ID 签名和 notarization。

## CI

`.github/workflows/ci.yml` 在 Windows、macOS、Linux 三个矩阵上跑：

1. Checkout（Windows 启用 `core.longpaths`）。
2. Linux 安装上述系统库。
3. Setup Bun + Rust stable。
4. `swatinem/rust-cache@v2` 缓存 `target`。
5. 运行 `bun run scripts/ci.js`。
6. `cargo build --workspace --release`。
7. 安装 Tauri CLI。
8. 按平台构建 bundle（NSIS / app+dmg / deb+appimage）。
9. 把 bundle 复制到 `dist/artifacts/dou-voice-{platform}-{target}-{type}.{ext}`。
10. 上传 artifact，名称形如 `dou-voice-{platform}-{target}-{type}.{ext}`（如 `dou-voice-windows-x86_64-pc-windows-msvc-nsis.exe`、`dou-voice-macos-aarch64-apple-darwin-dmg.dmg`、`dou-voice-linux-x86_64-unknown-linux-gnu-appimage.AppImage`）。

`.github/workflows/windows-build.yml` 由 `v*` tag 或手动触发，专门出 Windows NSIS installer。

## 平台验证边界

CI 只能证明「能编译、能打包、Rust 测试通过」。以下内容必须真机验收：

- 麦克风权限和采集（macOS 需要在 System Settings 授予）。
- 登录窗口的 Cookie / localStorage 提取（豆包前端结构可能随服务端更新而变化）。
- 全局热键 pressed/released 事件（Windows 用平台轮询；macOS/Linux 用 `tauri-plugin-global-shortcut`）。
- 文本输入模拟和剪贴板 fallback（不同输入法、不同桌面环境差异大）。
- overlay 层级、non-activating 行为、多显示器位置。
- macOS artifact 的未签名安装体验。

Windows 是主验证路径，详见 [Windows 使用与验收](windows-desktop.md)。macOS / Linux 的运行时行为目前主要由 CI 保证编译通过，真机验收仍是空白。

## 代码与文档约定

来自 [AGENTS.md](../AGENTS.md) 与 workspace lint：

- 中文回答用户；代码注释和文档可用中文；用户可见的错误、日志、事件名、状态名必须用英文。
- 写代码时加适量注释；单文件不超过 500 行，超过就要拆模块。
- 提交前用对应的 format 工具格式化（Rust 用 `cargo fmt`，前端由 Bun 构建管线处理）。
- 不提交 `auth.json`、诊断文件、录音样本、Cookie、token、设备标识或任何用户数据（`.gitignore` 已包含相关模式）。
- Workspace 强制 `unsafe_code = "forbid"`，`dbg_macro`、`todo`、`unwrap_used` 在 clippy 中 deny。
- 平台差异封进 `dou-voice-platform`（按 `cfg` 切换子模块），核心协议和状态机封进 `dou-voice-core`，Tauri shell 只做产品集成。
- 文档命令必须来自 manifest、CI、脚本或生态标准，不写猜测命令。

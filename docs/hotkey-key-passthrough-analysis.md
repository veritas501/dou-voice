# 热键按键透传分析：`Ctrl+Alt+Z` 在微信输入框出现 `Z`

**状态**：分析完成；Windows 已按方案 A 落地（`WH_KEYBOARD_LL` 吞主键），见 `windows_hotkey_hook.rs`  

**现象**：热键设置为 `Ctrl+Alt+Z` 时，在微信（或其他前台输入框）按住热键开始录音，输入框里仍会冒出字符 `Z`（用户描述为「大写 Z」）  
**结论（先说）**：是的，按键被**完整透传**给了前台窗口。这不是微信单独的 bug，而是当前 **Windows 热键实现「只观测、不吞键」** 的结构性结果。

---

## 1. 现象与复现路径

### 1.1 用户路径

1. 设置 press-to-talk 热键为 `Ctrl + Alt + Z`
2. 焦点在微信聊天输入框
3. 按住热键开始录音 / 松开结束
4. 输入框出现 `Z`（或 `z`，视 Caps Lock / IME / 应用对字符的渲染而定）

### 1.2 预期 vs 实际

| 维度 | 产品预期 | 当前行为 |
|------|----------|----------|
| 热键按下 | 开始录音，**不**向当前窗口注入/透传字符 | 开始录音，**同时**前台窗口仍收到 `Z` 相关按键消息 |
| 热键松开 | 结束录音并识别输入 | 结束录音正常；透传的字符已留在输入框 |
| 焦点 | 不抢焦点（设计目标之一） | 焦点仍在微信 → 透传字符更容易被看见 |

默认热键 `Ctrl+Q` 在多数输入框里通常**不会**插入字母 `Q`（被当作快捷键/被应用忽略），所以问题更不明显；换成带普通字母主键的组合（尤其 `Z`）后，透传就暴露了。

---

## 2. 当前热键架构（与透传直接相关）

### 2.1 平台分流

| 平台 | 入口 | 机制 | 是否拦截/吞掉按键 |
|------|------|------|-------------------|
| **Windows** | `apps/dou-voice-desktop/src-tauri/src/hotkey.rs` → `spawn_windows_modifier_hotkey_listener` | 后台线程每 **30ms** 调 `dou_voice_platform::hotkey::hotkey_pressed` | **否**（只读键盘状态） |
| **macOS** | `macos_hotkey.rs` → `CGEventTap` | `CGEventTapOptions::ListenOnly` | **否**（监听模式，事件原样返回） |
| **Linux** | Tauri `global-shortcut` 插件 | 系统快捷键注册 | 依赖后端；与本次 Windows/微信问题无关 |

Windows / macOS 两侧在「是否消费按键」上是一致的：**都没有消费**。

### 2.2 Windows 完整调用链

```text
setup_global_shortcut (windows)
  └─ spawn_windows_modifier_hotkey_listener
       loop every WINDOWS_HOTKEY_POLL_INTERVAL (30ms)
         hotkey = current_hotkey_or_default()
         next_pressed = dou_voice_platform::hotkey::hotkey_pressed(&hotkey)
         false→true  → trigger_hotkey_pressed  → start_hotkey_recording
         true→false  → trigger_hotkey_released → finish_hotkey_recording
```

关键点：

- `update_global_shortcut` 在 Windows/macOS 上是 **空操作**（`Ok(())`），**不会**调用 `RegisterHotKey` / Tauri global-shortcut 注册。
- 注释写明：Windows 用轮询，是为了支持 **modifier-only**（如 `Win+Alt`）等 Tauri/global-hotkey 字符串注册路径搞不定的组合。

### 2.3 `hotkey_pressed` 如何判断（只读）

`crates/dou-voice-platform/src/hotkey/windows_hotkey.rs`：

```text
hotkey_pressed("Ctrl+Alt+Z")
  → parse_hotkey → HotkeySpec { ctrl, alt, key: Letter('Z') }
  → GetAsyncKeyState(VK_CONTROL/VK_LCONTROL/VK_RCONTROL) < 0
  → GetAsyncKeyState(VK_MENU/VK_LMENU/VK_RMENU) < 0
  → GetAsyncKeyState(VK_Z) < 0
```

`GetAsyncKeyState` **只查询硬件/异步键态**，不：

- 安装 `WH_KEYBOARD` / `WH_KEYBOARD_LL` hook
- 调用 `RegisterHotKey`
- 调用 `BlockInput` / 过滤 `WM_KEYDOWN`
- 修改或丢弃任何输入消息

因此系统仍会把 `Ctrl/Alt/Z` 的 key-down / key-up 送到**前台窗口消息队列**（微信）。

### 2.4 macOS 对照（同样不吞）

`macos_hotkey.rs` 创建 tap 时使用：

```text
CGEventTapOptions::ListenOnly
```

回调末尾一律 `return event.as_ptr()`，即事件继续向下传递。macOS 上同类「字母主键热键」理论上也会透传；只是本次反馈场景是微信 + Windows。

### 2.5 与「输入阶段」清理逻辑的区别

`crates/dou-voice-platform/src/input/windows_input.rs` 在**识别结束后打字前**会：

- 等待热键修饰键自然松开
- 补发残留修饰键 key-up
- 发 ESC key-up 清菜单激活态

这只保护 **ASR 结果注入**，**不会**阻止热键按下瞬间前台窗口已经收到的 `Z`。时间线：

```text
t0  用户按下 Ctrl+Alt+Z
t0+ 微信消息队列已收到按键（可能插入 Z）
t0~30ms  轮询线程发现组合键按下 → 开始录音
...
tn  用户松开
tn+ 结束录音 → ASR → type_text（此时才做修饰键清理）
```

透传发生在 **t0**，清理发生在 **tn 之后**，两段逻辑互不覆盖。

---

## 3. 根因判断

### 3.1 直接根因

> **Windows press-to-talk 是「轮询观测」模型，不是「系统级独占/消费」模型。**  
> 热键匹配成功只驱动录音状态机，**从不从输入流里拿走该次按键**。

所以用户看到的「大写 Z 出现在输入框」= **真实物理按键被前台应用正常消费**，不是 Dou Voice 用 `SendInput` 又打了一遍 `Z`（热键路径里没有往前台注入主键的代码）。

### 3.2 为什么是 `Z`，而不是整段 `Ctrl+Alt+Z` 的「快捷键效果」

- `Ctrl` / `Alt` 单独通常不产生可打印字符。
- 主键 `Z` 的 key-down 在「修饰键已按下」时，**部分应用**会当快捷键；**部分应用 / IME** 仍会向输入框插入字母。
- 微信 PC 输入框 + 中文输入法 对 `Ctrl+Alt+字母` 的处理并不统一：常见结果就是 **字母落到输入框**，或松开顺序不当导致短暂变成「裸 Z」。
- 用户说「大写 Z」的可能来源（可并存，不互斥）：
  1. Caps Lock 开启
  2. 口语里把字母键统称「大写 Z」（界面显示实为 `z`）
  3. 某输入法/皮肤对西文字符强制大写显示
  4. 释放顺序导致 Shift 态偶发参与（次要）

无论大小写，**本质都是主键字符事件到达了微信**。

### 3.3 为什么默认 `Ctrl+Q` 较少暴露

- 很多程序把 `Ctrl+Q` 当退出/其他命令，或直接忽略可打印输出。
- 输入框里插入 `q` 的概率低于 `z`（也依赖应用）。
- 文档与验收清单默认用 `Ctrl+Q` + Notepad，**没有覆盖「字母热键 + IM 输入框」场景**。

### 3.4 可排除的方向

| 假设 | 是否成立 | 依据 |
|------|----------|------|
| 识别结果误带了 `Z` | 否 | 按键瞬间就出现；与 ASR final 时序无关 |
| `enigo` / 剪贴板粘贴误输入 `Z` | 否 | 输入逻辑在录音结束后；且不会只打一个无上下文的 `Z` |
| 热键注册失败导致「半工作」 | 否 | 录音仍开始 → 轮询检测是成功的；问题在「检测 ≠ 吞键」 |
| 仅微信 bug | 弱 | 任何把字母 key 当字符的前台窗口都可能复现；微信只是高频场景 |
| macOS ListenOnly 不同 | 同构 | macOS 同样不吞；Windows 更「裸读」而已 |

---

## 4. 设计层面的张力

当前产品有两条目标，在实现上互相拉扯：

1. **不抢焦点 / press-to-talk**：用户在任意窗口说话，光标留在原处。
2. **热键「干净」**：按热键时前台应用不应收到副作用字符或错误快捷键。

现有实现优先保证了 (1) 的「观察即可工作、支持 modifier-only」，但 **完全没有实现 (2)**。  
架构注释也承认：Windows 轮询是为了 **modifier-only 注册能力**，不是为了 **键事件独占**。

另外，文本输入路径已经意识到修饰键残留问题（`release_stuck_modifiers` 等），说明团队知道「热键与目标窗口共享键盘流」的风险，但治理点放在 **输出阶段**，没有放在 **热键触发阶段**。

---

## 5. 证据索引（代码）

| 文件 | 要点 |
|------|------|
| `apps/dou-voice-desktop/src-tauri/src/hotkey.rs` | Windows：`spawn_windows_modifier_hotkey_listener` 30ms 轮询；`update_global_shortcut` 空实现 |
| `crates/dou-voice-platform/src/hotkey/windows_hotkey.rs` | 仅 `GetAsyncKeyState`，无 hook / 无 `RegisterHotKey` |
| `crates/dou-voice-platform/src/hotkey.rs` | 解析 `Ctrl+Alt+Z` → `Letter('Z')`；`hotkey_pressed` 平台封装 |
| `apps/dou-voice-desktop/src-tauri/src/macos_hotkey.rs` | `ListenOnly` + 原样返回事件 |
| `crates/dou-voice-platform/src/input/windows_input.rs` | 仅 post-ASR 清理修饰键，不防热键透传 |
| `docs/architecture.md` / `docs/windows-desktop.md` | 文档写明 Windows 轮询与 debounce/busy 抑制，**未提吞键** |

---

## 6. 解决方案方向（供下一阶段选型，本文不落地）

按「侵入性 / 可靠性 / 与现有 modifier-only 兼容」排序建议：

### 方案 A：Windows 低级键盘钩子（`WH_KEYBOARD_LL`）按匹配组合吞键（推荐主路径）

- **做法**：安装 LL hook；当当前配置热键的 modifier+key 全部满足时，对**该次** key-down/key-up（至少主键 `Z`，视策略也可含修饰键）返回非零，阻止继续派发。
- **优点**：真正解决透传；与「不抢焦点」兼容；可同时覆盖字母键与部分危险组合。
- **风险**：需谨慎处理：设置页捕获热键、权限/杀软、钩子超时被系统摘掉、与其他 hook 软件冲突；必须在匹配失败时零延迟放行。
- **与轮询关系**：可用 hook 事件替代 30ms 轮询（降延迟、更准），或 hook 只负责 swallow、轮询仍驱动状态机（短期过渡）。

### 方案 B：`RegisterHotKey` / Tauri global-shortcut 注册带主键的组合

- **优点**：系统级注册通常会消费该组合，前台收不到。
- **缺点**：
  - 当前 Windows 有意避开这条路径，因为 **modifier-only** 和部分组合支持差；
  - press-to-talk 需要可靠的 **press + release**；`RegisterHotKey` 语义偏「触发一次」，release 要另做；
  - 与现有「保存设置后下一轮轮询即生效」模型不一致。
- **适用**：可作为「带主键热键」的补充路径，modifier-only 仍走轮询/hook。

### 方案 C：仅文档/产品规避（弱缓解）

- 引导用户使用不易产生字符的组合：`Ctrl+Alt`（modifier-only）、`Ctrl+Alt+Space`、功能键等。
- **不能**算修复：用户已选 `Ctrl+Alt+Z` 是合法设置，产品已支持字母主键。

### 方案 D：检测到热键后对前台「补删」`Z`（不推荐）

- 例如热键触发后向微信发 Backspace。
- 竞态大、误删用户原有字符、IME 组合态下更糟。只作极端 fallback，不作主方案。

### 方案 E：macOS 一并改为 `Default` tap 并在匹配时返回 `null` 吞掉

- 与 Windows 方案 A 对称，避免平台行为分裂。
- 需要辅助功能权限（已有）；注意与系统快捷键、输入法的优先级。

### 建议落地顺序（下一阶段）

1. **确认复现矩阵**：微信 / 记事本 / 浏览器输入框；`Ctrl+Alt+Z`、`Ctrl+Q`、`Ctrl+Alt`（modifier-only）；中/英输入法。
2. **Windows 实现 LL hook 吞主键**（最少吞匹配热键的 **key-down/key-up of main key**；修饰键是否吞可二期）。
3. 设置页 capture 期间禁用吞键（已有 `capture_active` 可复用）。
4. 诊断日志增加 `hotkey_swallowed` / hook 状态，便于验收。
5. 评估是否用 hook 事件替换 30ms 轮询（延迟与 CPU）。
6. macOS 是否同步改为可吞键，单独开任务。

---

## 7. 验收标准（修复后）

- 焦点在微信输入框，热键 `Ctrl+Alt+Z`：按住→说话→松开，**输入框不出现 `Z`/`z`**，识别文本仍正常插入。
- 默认 `Ctrl+Q`、modifier-only（若启用）行为不回归。
- 设置页「Change hotkey」仍能捕获新组合。
- 热键忙时 suppress、30ms debounce、release fallback 行为保持。
- 目标窗口焦点不被 overlay/主窗口抢走（现有不抢焦点约定）。

---

## 8. 总结

| 问题 | 答案 |
|------|------|
| 是不是有按键被透传？ | **是。** Windows 上整组 `Ctrl+Alt+Z` 都未从系统输入流中移除。 |
| 根因是什么？ | 热键实现 = `GetAsyncKeyState` **旁路观测**，无 hook / 无 `RegisterHotKey` 消费。 |
| 为何微信里看到 `Z`？ | 前台窗口正常收到主键消息；微信/IME 将其表现为输入字符。 |
| 是否 Dou Voice 二次注入了 `Z`？ | **否。** 是系统把真实按键交给了微信。 |
| 怎么修？ | 优先 **低级键盘钩子在匹配时吞键**；规避性改热键只是权宜之计。 |

---

*文档目的：先对齐根因与方案边界；实现与补丁在确认方案后再做。*


---

## 9. 实现记录（方案 A）

**落地位置**

- `crates/dou-voice-platform/src/hotkey/windows_hotkey_hook.rs`：LL hook 安装/消息泵/吞键判定
- `crates/dou-voice-platform/src/hotkey.rs`：对外 `start/stop/set_swallowed_hotkey/set_hotkey_swallow_enabled`
- `apps/dou-voice-desktop/src-tauri/src/hotkey.rs`：启动时装钩 + 同步热键；设置页 capture 时关闭吞键；轮询到热键变更时更新目标

**吞键规则（防误伤）**

1. 只吞配置热键的 **主键** down/up，从不吞 Ctrl/Alt/Shift/Win
2. 修饰键与配置 **精确一致**（未配置的修饰键必须未按下）→ 普通输入、`Ctrl+Z` 撤销、`Ctrl+V` 粘贴不受影响
3. modifier-only 热键不吞任何键
4. `LLKHF_INJECTED` 放行，避免干扰 enigo/剪贴板模拟输入
5. 钩子失败 fail-open：录音热键仍走轮询
6. 已吞下的主键 key-up 会继续吞，避免前台收到不对称事件


**踩坑**：`WH_KEYBOARD_LL` 吞掉主键后，`GetAsyncKeyState` 对该主键常返回未按下。  
轮询必须合并钩子内 `SWALLOWED_MAIN_VK`（`is_swallowed_main_key_held`），否则 press-to-talk 不会触发。

**编译验证**

- `cargo.exe check -p dou-voice-platform -p dou-voice-desktop --target x86_64-pc-windows-msvc` 通过
- `cargo.exe test -p dou-voice-platform --target x86_64-pc-windows-msvc --lib` 12 passed

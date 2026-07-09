//! 平台能力适配层。
//!
//! 该 crate 暴露核心产品需要、但不能放进 `dou-voice-core` 的 OS 相关能力。当前优先实现
//! Windows 直接输入；macOS/Linux 后续应在这里增加各自模块，并保持对上层的接口稳定。

pub mod feedback;
pub mod hotkey;
pub mod input;

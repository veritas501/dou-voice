use crate::{CoreError, CoreResult};

/// 语音输入状态机的稳定状态。
///
/// 该状态机只描述核心录音/识别生命周期，不包含托盘、音效、overlay 等 UI 细节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionState {
    /// 等待触发。
    Idle,
    /// 已收到开始命令，正在准备录音或连接 ASR。
    Starting,
    /// 正在录音。
    Recording,
    /// 已收到停止命令，正在完成识别。
    Stopping,
}

impl TranscriptionState {
    /// 返回稳定的英文状态名，用于日志、UI 和错误信息。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Recording => "recording",
            Self::Stopping => "stopping",
        }
    }
}

/// 状态机可接受的输入命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionCommand {
    /// 用户触发切换；在 idle 表示开始，在 recording 表示停止。
    Toggle,
    /// ASR 连接已建立或录音准备完成。
    Connected,
    /// 平台层接受停止请求。
    StopAccepted,
    /// 识别流程完成。
    Finished,
    /// 用户取消。
    Cancel,
    /// 流程失败。
    Fail,
}

impl TranscriptionCommand {
    /// 返回稳定的英文命令名，用于错误信息。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Connected => "connected",
            Self::StopAccepted => "stop_accepted",
            Self::Finished => "finished",
            Self::Cancel => "cancel",
            Self::Fail => "fail",
        }
    }
}

/// 最小核心状态机。
///
/// 平台层负责把热键 press/release、托盘菜单和错误映射为命令；核心层只校验状态转换。
#[derive(Debug, Clone)]
pub struct TranscriptionStateMachine {
    state: TranscriptionState,
}

impl Default for TranscriptionStateMachine {
    fn default() -> Self {
        Self {
            state: TranscriptionState::Idle,
        }
    }
}

impl TranscriptionStateMachine {
    /// 创建一个处于 idle 状态的状态机。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前状态。
    pub fn state(&self) -> TranscriptionState {
        self.state
    }

    /// 应用命令并返回新状态。
    pub fn apply(&mut self, command: TranscriptionCommand) -> CoreResult<TranscriptionState> {
        let next = match (self.state, command) {
            (TranscriptionState::Idle, TranscriptionCommand::Toggle) => {
                TranscriptionState::Starting
            }
            (TranscriptionState::Starting, TranscriptionCommand::Connected) => {
                TranscriptionState::Recording
            }
            (TranscriptionState::Recording, TranscriptionCommand::Toggle) => {
                TranscriptionState::Stopping
            }
            (TranscriptionState::Stopping, TranscriptionCommand::Finished) => {
                TranscriptionState::Idle
            }
            (_, TranscriptionCommand::Cancel | TranscriptionCommand::Fail) => {
                TranscriptionState::Idle
            }
            (TranscriptionState::Starting, TranscriptionCommand::StopAccepted)
            | (TranscriptionState::Recording, TranscriptionCommand::StopAccepted) => {
                TranscriptionState::Stopping
            }
            _ => {
                return Err(CoreError::InvalidState {
                    current: self.state.as_str(),
                    command: command.as_str(),
                });
            }
        };

        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::{TranscriptionCommand, TranscriptionState, TranscriptionStateMachine};

    #[test]
    fn follows_happy_path() {
        let mut machine = TranscriptionStateMachine::new();

        assert_eq!(
            machine.apply(TranscriptionCommand::Toggle),
            Ok(TranscriptionState::Starting)
        );
        assert_eq!(
            machine.apply(TranscriptionCommand::Connected),
            Ok(TranscriptionState::Recording)
        );
        assert_eq!(
            machine.apply(TranscriptionCommand::Toggle),
            Ok(TranscriptionState::Stopping)
        );
        assert_eq!(
            machine.apply(TranscriptionCommand::Finished),
            Ok(TranscriptionState::Idle)
        );
    }

    #[test]
    fn cancel_returns_to_idle_from_any_state() {
        let mut machine = TranscriptionStateMachine::new();
        let _ = machine.apply(TranscriptionCommand::Toggle);

        assert_eq!(
            machine.apply(TranscriptionCommand::Cancel),
            Ok(TranscriptionState::Idle)
        );
    }
}

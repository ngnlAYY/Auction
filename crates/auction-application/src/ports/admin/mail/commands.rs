//! Admin Mail owner 的状态命令和命令账本模型。

use auction_domain::CommandId;
use auction_domain::valid_optional_text;

use super::errors::AdminMailError;
use super::limits::{MAX_ADMIN_MAILBOX_KEY_BYTES, valid_text};
use crate::ports::core::RequestHash;

#[derive(Debug, Clone)]
pub struct AdminMailPollingCommand {
    pub command_id: CommandId,
    pub mailbox_key: String,
    pub paused: bool,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminMailPollingCommand {
    /// 校验不依赖数据库当前状态的命令字段。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        if self.command_id.is_nil()
            || self.expected_state_version <= 0
            || self.expected_state_version == i64::MAX
            || self.request_hash.as_bytes() == [0; 32]
            || !valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            || !valid_text(&self.actor_id, 512)
            || !valid_optional_text(self.request_id.as_deref(), 512, true)
        {
            return Err(AdminMailError::InvalidCommand);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AdminMailPollTriggerCommand {
    pub command_id: CommandId,
    pub mailbox_key: String,
    /// `0` 只用于首次建立邮箱状态；已有状态必须携带当前版本。
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminMailPollTriggerCommand {
    /// 校验不依赖数据库当前状态的触发命令字段。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        if self.command_id.is_nil()
            || self.expected_state_version < 0
            || self.expected_state_version == i64::MAX
            || self.request_hash.as_bytes() == [0; 32]
            || !valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            || !valid_text(&self.actor_id, 512)
            || !valid_optional_text(self.request_id.as_deref(), 512, true)
        {
            return Err(AdminMailError::InvalidCommand);
        }
        Ok(())
    }
}

/// 邮件 Admin 命令账本中可安全重放的最小记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminMailStoredCommand {
    pub command_id: CommandId,
    pub command_kind: String,
    pub mailbox_key: String,
    pub request_hash: RequestHash,
    pub result_payload: Vec<u8>,
}

impl AdminMailStoredCommand {
    /// 校验账本记录，防止把损坏结果交给 HTTP 重放。
    pub fn validate(&self) -> Result<(), AdminMailError> {
        if self.command_id.is_nil()
            || !matches!(self.command_kind.as_str(), "set_polling_paused" | "trigger_poll")
            || !valid_text(&self.mailbox_key, MAX_ADMIN_MAILBOX_KEY_BYTES)
            || self.request_hash.as_bytes() == [0; 32]
            || self.result_payload.is_empty()
            || serde_json::from_slice::<serde_json::Value>(&self.result_payload)
                .ok()
                .is_none_or(|value| !value.is_object())
        {
            return Err(AdminMailError::StoredResultCorrupt);
        }
        Ok(())
    }

    /// 判断账本记录是否对应同一暂停命令。
    pub fn matches_request(&self, command: &AdminMailPollingCommand) -> bool {
        self.command_kind == "set_polling_paused"
            && self.mailbox_key == command.mailbox_key
            && self.request_hash == command.request_hash
    }

    /// 判断账本记录是否对应同一立即触发命令。
    pub fn matches_trigger_request(&self, command: &AdminMailPollTriggerCommand) -> bool {
        self.command_kind == "trigger_poll"
            && self.mailbox_key == command.mailbox_key
            && self.request_hash == command.request_hash
    }
}

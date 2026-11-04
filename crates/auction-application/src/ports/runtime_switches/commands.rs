//! 稳定命令、摘要、版本 CAS 与到期收口规则。

use super::{RuntimeSwitchError, RuntimeSwitchName};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 命令及事实动作；到期也只追加。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSwitchOperation {
    /// 设置覆盖。
    Set,
    /// 清除覆盖。
    Clear,
    /// 对已到期的最新设置追加收口事实。
    Expire,
}

impl RuntimeSwitchOperation {
    /// SQL 与审计共用的固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Clear => "clear",
            Self::Expire => "expire",
        }
    }
}

/// 可重放的接收结果，同时是只追加事实。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSwitchEvent {
    /// 非零命令身份。
    pub command_id: Uuid,
    /// 已知开关名。
    pub name: RuntimeSwitchName,
    /// 单名称递增版本。
    pub revision: i64,
    /// 动作。
    pub operation: RuntimeSwitchOperation,
    /// set 的类型化值；其他动作无值。
    pub value: Option<Value>,
    /// set 的数据库期限。
    pub expires_at: Option<DateTime<Utc>>,
    /// expire 对应的设置命令。
    pub expired_command_id: Option<Uuid>,
    /// 操作者声明；Admin 使用会话身份。
    pub actor_id: String,
    /// 实际命令入口。
    pub entrypoint: String,
    /// 单行非敏感原因。
    pub reason: String,
    /// 数据库接收时间。
    pub recorded_at: DateTime<Utc>,
}

impl RuntimeSwitchEvent {
    /// 校验可读事实，不用当前时间拒绝合法的历史过期记录。
    pub fn validate(&self) -> Result<(), RuntimeSwitchError> {
        let shape = match self.operation {
            RuntimeSwitchOperation::Set => {
                self.value.as_ref().is_some_and(|value| self.name.valid_value(value))
                    && self.expired_command_id.is_none()
                    && self.expires_at.is_none_or(|until| until > self.recorded_at)
            }
            RuntimeSwitchOperation::Clear => {
                self.value.is_none()
                    && self.expires_at.is_none()
                    && self.expired_command_id.is_none()
            }
            RuntimeSwitchOperation::Expire => {
                self.value.is_none()
                    && self.expires_at.is_none()
                    && self.expired_command_id.is_some_and(|id| !id.is_nil())
            }
        };
        if !shape
            || self.command_id.is_nil()
            || self.revision < 1
            || !text(&self.actor_id, 256)
            || !text(&self.reason, 1024)
            || !matches!(self.entrypoint.as_str(), "admin" | "cli" | "system")
        {
            return Err(RuntimeSwitchError::Corrupt);
        }
        Ok(())
    }
}

/// 稳定命令负载；摘要不包含易变的请求时间或刷新状态。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSwitchCommand {
    /// 原样重试所需的调用方命令 ID。
    pub command_id: Uuid,
    /// 名称。
    pub name: RuntimeSwitchName,
    /// 预期最新版本；不存在时为零。
    pub expected_revision: i64,
    /// 动作。
    pub operation: RuntimeSwitchOperation,
    /// set 值。
    pub value: Option<Value>,
    /// 绝对期限，不在重试时重新计算。
    pub expires_at: Option<DateTime<Utc>>,
    /// 系统到期命令引用的原设置。
    pub expired_command_id: Option<Uuid>,
    /// 操作者。
    pub actor_id: String,
    /// 实际入口，加入摘要防止重放时篡改来源。
    pub entrypoint: String,
    /// 必填原因。
    pub reason: String,
}

impl RuntimeSwitchCommand {
    /// 基础校验不依赖现在时刻，保证过期命令仍可重放。
    pub fn validate(&self) -> Result<(), RuntimeSwitchError> {
        if self.name == RuntimeSwitchName::MetricsRejectForwardedRequests
            && self.operation == RuntimeSwitchOperation::Set
            && self.value.as_ref().and_then(Value::as_bool) == Some(false)
        {
            // 安全策略只允许在线收紧；放宽仍需修改经过部署审阅的 TOML 并重启。
            return Err(RuntimeSwitchError::UnsafeRelaxation);
        }
        if self.expires_at.is_some_and(|until| {
            until.timestamp_subsec_nanos() >= 1_000_000_000
                || until.timestamp_subsec_nanos() % 1000 != 0
        }) {
            return Err(RuntimeSwitchError::Invalid);
        }
        let event = self.event(Utc::now())?;
        // 期限只对新命令在数据库锁内核对；此处允许已到期请求先查账本。
        let mut historical = event;
        historical.expires_at = None;
        historical.validate().map_err(invalid)?;
        if self.operation == RuntimeSwitchOperation::Expire && self.entrypoint != "system" {
            return Err(RuntimeSwitchError::Invalid);
        }
        Ok(())
    }

    /// 应用层统一版本化摘要，CLI 与未来 HTTP 共用。
    pub fn digest(&self) -> Result<[u8; 32], RuntimeSwitchError> {
        crate::RequestHash::from_request("runtime_switch_v1", self.command_id, self)
            .map(crate::RequestHash::as_bytes)
            .map_err(invalid)
    }

    /// 验证账本身份与固定结果；重放不重新检查期限或当前版本。
    pub fn replay(
        &self,
        digest_version: i16,
        request_digest: &[u8],
        event: RuntimeSwitchEvent,
    ) -> Result<RuntimeSwitchEvent, RuntimeSwitchError> {
        if digest_version != 1 || request_digest != self.digest()? {
            return Err(RuntimeSwitchError::Conflict);
        }
        event.validate()?;
        if event.command_id != self.command_id
            || event.name != self.name
            || event.revision
                != self.expected_revision.checked_add(1).ok_or(RuntimeSwitchError::Corrupt)?
            || event.operation != self.operation
            || event.value != self.value
            || event.expires_at != self.expires_at
            || event.expired_command_id != self.expired_command_id
            || event.actor_id != self.actor_id
            || event.entrypoint != self.entrypoint
            || event.reason != self.reason
        {
            return Err(RuntimeSwitchError::Corrupt);
        }
        Ok(event)
    }

    /// 在锁内构造下一事实；不修改原事实或旧结果。
    pub fn accept(
        &self,
        previous: Option<&RuntimeSwitchEvent>,
        now: DateTime<Utc>,
    ) -> Result<RuntimeSwitchEvent, RuntimeSwitchError> {
        if let Some(previous) = previous {
            previous.validate()?;
            if previous.name != self.name {
                return Err(RuntimeSwitchError::Corrupt);
            }
        }
        if previous.map_or(0, |event| event.revision) != self.expected_revision {
            return Err(RuntimeSwitchError::Conflict);
        }
        if self.operation == RuntimeSwitchOperation::Expire {
            if !previous.is_some_and(|event| {
                event.operation == RuntimeSwitchOperation::Set
                    && Some(event.command_id) == self.expired_command_id
                    && event.expires_at.is_some_and(|until| until <= now)
            }) {
                return Err(RuntimeSwitchError::Conflict);
            }
        } else if self.expires_at.is_some_and(|until| until <= now) {
            return Err(RuntimeSwitchError::Invalid);
        }
        let event = self.event(now)?;
        event.validate().map_err(invalid)?;
        Ok(event)
    }

    fn event(&self, recorded_at: DateTime<Utc>) -> Result<RuntimeSwitchEvent, RuntimeSwitchError> {
        let revision = self
            .expected_revision
            .checked_add(1)
            .filter(|revision| *revision > 0)
            .ok_or(RuntimeSwitchError::Invalid)?;
        Ok(RuntimeSwitchEvent {
            command_id: self.command_id,
            name: self.name,
            revision,
            operation: self.operation,
            value: self.value.clone(),
            expires_at: self.expires_at,
            expired_command_id: self.expired_command_id,
            actor_id: self.actor_id.clone(),
            entrypoint: self.entrypoint.clone(),
            reason: self.reason.clone(),
            recorded_at,
        })
    }
}

pub(super) fn text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn invalid<T>(_: T) -> RuntimeSwitchError {
    RuntimeSwitchError::Invalid
}

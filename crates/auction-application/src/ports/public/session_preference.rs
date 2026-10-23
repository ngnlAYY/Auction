//! 账号级 Public 登录时长、固定截止计算与会话绑定的幂等请求；默认永久。

use auction_domain::CommandId;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{PublicSessionIdentity, RequestHash, SessionError};

/// 后续登录采用的账号偏好；独立版本从 1 开始，不改变用户认证版本。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PublicSessionPreference {
    pub user_id: Uuid,
    /// `None` 表示服务端无固定到期；有限值必须为 1 分钟至 365 天的整分钟数。
    pub duration_seconds: Option<i64>,
    pub version: i64,
}

impl PublicSessionPreference {
    /// 校验应用输入和持久化投影共用的偏好边界。
    pub fn validate(&self) -> Result<(), SessionError> {
        if self.user_id.is_nil() || self.version <= 0 {
            return Err(SessionError::InvalidPreference);
        }
        validate_duration(self.duration_seconds)
    }

    /// 以本次服务器时间计算固定截止，不使用上次活动时间或部署级 Public TTL。
    pub fn expires_at_from(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, SessionError> {
        self.validate()?;
        self.duration_seconds
            .map(|seconds| {
                now.checked_add_signed(Duration::seconds(seconds))
                    .ok_or(SessionError::InvalidPreference)
            })
            .transpose()
    }
}

/// 偏好与当前会话实际截止；旧会话及其他设备可以保留不同截止。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicSessionPreferenceResult {
    pub preference: PublicSessionPreference,
    pub identity: PublicSessionIdentity,
}

impl PublicSessionPreferenceResult {
    /// 读取及历史账本投影必须保持用户绑定和双截止形状，不能把混合空值解释成永久。
    pub fn validate(&self) -> Result<(), SessionError> {
        self.preference.validate()?;
        let deadlines_valid = match (self.identity.expires_at, self.identity.absolute_expires_at) {
            (None, None) => true,
            (Some(expires_at), Some(absolute_expires_at)) => expires_at <= absolute_expires_at,
            _ => false,
        };
        if self.identity.user_id != self.preference.user_id
            || self.identity.username.is_empty()
            || !deadlines_valid
        {
            return Err(SessionError::InvalidIdentity);
        }
        Ok(())
    }
}

/// 稳定命令不接受客户端时间或 token；应用服务另行绑定当前认证会话摘要。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SetPublicSessionPreferenceCommand {
    pub command_id: CommandId,
    pub user_id: Uuid,
    pub duration_seconds: Option<i64>,
    pub expected_version: i64,
}

impl SetPublicSessionPreferenceCommand {
    /// 既有共享命令账本中的固定命令种类。
    pub const KIND: &'static str = "set_public_session_preference";

    /// 校验命令身份、版本和有限时长；同一时长保存仍推进一次偏好版本。
    pub fn validate(&self) -> Result<(), SessionError> {
        if self.command_id.is_nil() || self.user_id.is_nil() || self.expected_version <= 0 {
            return Err(SessionError::InvalidPreference);
        }
        validate_duration(self.duration_seconds)
    }

    /// 请求摘要绑定用户、当前会话、完整负载和命令 ID；摘要中不含本次重试时间。
    pub fn request_hash(&self, token_hash: [u8; 32]) -> Result<RequestHash, SessionError> {
        self.validate()?;
        RequestHash::from_request(Self::KIND, self.user_id, &(self, token_hash))
            .map_err(|_| SessionError::InvalidPreference)
    }

    /// 在用户锁内完成版本比较并生成新偏好；其他会话不参与本次更新。
    pub fn next_preference(
        &self,
        current: PublicSessionPreference,
    ) -> Result<PublicSessionPreference, SessionError> {
        self.validate()?;
        current.validate()?;
        if current.user_id != self.user_id || current.version != self.expected_version {
            return Err(SessionError::PreferenceConflict);
        }
        let version = current.version.checked_add(1).ok_or(SessionError::PreferenceConflict)?;
        Ok(PublicSessionPreference {
            user_id: self.user_id,
            duration_seconds: self.duration_seconds,
            version,
        })
    }
}

fn validate_duration(duration_seconds: Option<i64>) -> Result<(), SessionError> {
    if duration_seconds
        .is_some_and(|seconds| !(60..=31_536_000).contains(&seconds) || seconds % 60 != 0)
    {
        return Err(SessionError::InvalidPreference);
    }
    Ok(())
}

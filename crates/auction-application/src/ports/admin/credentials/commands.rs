//! Admin 凭据管理的命令闭集与写入计划。

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::value::{
    AdminCredential, AdminCredentialEnvelope, AdminCredentialKind, AdminCredentialStatus,
    AdminCredentialWorld,
};
use crate::ports::admin::AdminCredentialImportSource;
use crate::ports::core::RequestHash;

/// 凭据管理命令闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCredentialCommandKind {
    Import,
    Rename,
    SetStatus,
    Delete,
}

impl AdminCredentialCommandKind {
    /// 返回命令账本与审计使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Rename => "rename",
            Self::SetStatus => "set_status",
            Self::Delete => "delete",
        }
    }
}

/// 凭据导入命令；明文只在当前命令生命周期内存在。
pub struct AdminCredentialImportCommand {
    pub command_id: Uuid,
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub label: String,
    pub player_name: Option<String>,
    pub source: AdminCredentialImportSource,
    pub actor_id: String,
    /// CLI 装配根采集的身份；Admin 入口不提供。
    pub operator_identity: Option<crate::ports::core::CliOperatorIdentity>,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

impl fmt::Debug for AdminCredentialImportCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentialImportCommand")
            .field("command_id", &self.command_id)
            .field("kind", &self.kind)
            .field("world", &self.world)
            .field("label", &self.label)
            .field("source", &self.source)
            .field("actor_id", &self.actor_id)
            .field("request_id", &self.request_id)
            .field("now", &self.now)
            .finish()
    }
}

/// 凭据标签修改命令。
#[derive(Debug, Clone)]
pub struct AdminCredentialRenameCommand {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub label: String,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 凭据状态修改命令。
#[derive(Debug, Clone)]
pub struct AdminCredentialStatusCommand {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub status: AdminCredentialStatus,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 凭据删除命令。
#[derive(Debug, Clone)]
pub struct AdminCredentialDeleteCommand {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Admin 凭据导入写入；只含密文信封。
pub struct AdminCredentialImportWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub credential_id: Uuid,
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub label: String,
    pub player_name: Option<String>,
    pub envelope: AdminCredentialEnvelope,
    pub actor_id: String,
    pub operator_identity: Option<crate::ports::core::CliOperatorIdentity>,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Admin 凭据标签/状态修改写入。
#[derive(Debug)]
pub struct AdminCredentialMutationWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub action: AdminCredentialCommandKind,
    pub label: Option<String>,
    pub status: Option<AdminCredentialStatus>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Admin 凭据删除写入。
#[derive(Debug)]
pub struct AdminCredentialDeleteWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 删除命令的安全重放结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredentialDeleteResult {
    pub credential_id: Uuid,
    pub state_version: i64,
}

impl AdminCredentialDeleteResult {
    /// 校验删除结果，不依赖已删除的凭据行。
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil() && self.state_version > 1
    }
}

/// 凭据命令账本中的安全结果；删除后 credential 必须为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialCommandResult {
    pub credential: Option<AdminCredential>,
    pub deleted: Option<AdminCredentialDeleteResult>,
}

impl AdminCredentialCommandResult {
    /// 校验命令结果的互斥形状和安全投影。
    pub fn validate(&self) -> bool {
        matches!((&self.credential, &self.deleted), (Some(credential), None) if credential.validate())
            || matches!((&self.credential, &self.deleted), (None, Some(deleted)) if deleted.validate())
    }
}

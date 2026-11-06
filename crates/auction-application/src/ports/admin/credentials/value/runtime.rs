//! Worker 使用的凭据运行时安全记录。

use std::fmt;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{model::AdminCredentialKind, secret::AdminCredentialEnvelope};

/// Worker 读取的单条 active 凭据记录；仍不包含明文。
pub struct AdminCredentialRuntimeRecord {
    pub id: Uuid,
    pub kind: AdminCredentialKind,
    pub scope: String,
    pub state_version: i64,
    pub player_name: Option<String>,
    pub envelope: AdminCredentialEnvelope,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for AdminCredentialRuntimeRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentialRuntimeRecord")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("scope", &self.scope)
            .field("envelope", &self.envelope)
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl AdminCredentialRuntimeRecord {
    /// 校验 Worker 读取的内部 scope、密文信封和稳定排序字段。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && self.state_version > 0
            && crate::ports::admin::valid_credential_player_name(
                self.kind,
                self.player_name.as_deref(),
            )
            && self.scope == self.scope.trim()
            && !self.scope.is_empty()
            && self.scope.starts_with(self.kind.scope_prefix())
            && self.created_at.timestamp_nanos_opt().is_some()
            && self.envelope.ciphertext().len() >= 16
            && self.envelope.nonce().len() == 12
    }
}

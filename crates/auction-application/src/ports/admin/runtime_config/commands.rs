//! runtime 更新命令、命令账本状态与审计记录。

use auction_domain::CommandId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    document::{ADMIN_RUNTIME_DIGEST_VERSION, AdminRuntimeDocument},
    errors::AdminRuntimeConfigError,
};
use crate::ports::core::RequestHash;

/// Admin runtime 更新命令；HTTP、Worker 和 CLI 入口都复用这组 fence/摘要语义。
#[derive(Debug, Clone)]
pub struct AdminRuntimeUpdateCommand {
    pub command_id: CommandId,
    pub expected_fingerprint: String,
    pub document: AdminRuntimeDocument,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminRuntimeUpdateCommand {
    /// 校验命令的输入边界和版本化请求摘要。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        if self.command_id.is_nil()
            || !super::validation::valid_fingerprint(&self.expected_fingerprint)
            || !super::validation::valid_text(&self.actor_id, 256)
            || self
                .request_id
                .as_deref()
                .is_some_and(|value| !super::validation::valid_text(value, 256))
            || self.document.validate().is_err()
        {
            return Err(AdminRuntimeConfigError::InvalidCommand(
                "runtime 更新命令字段无效".to_owned(),
            ));
        }
        if self.document.request_hash(&self.expected_fingerprint)? != self.request_hash {
            return Err(AdminRuntimeConfigError::InvalidCommand(
                "runtime 更新命令摘要不匹配".to_owned(),
            ));
        }
        Ok(())
    }
}

/// durable runtime 命令的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminRuntimeCommandState {
    Pending,
    Applied,
    Unknown,
}

/// runtime 文件完成替换后、数据库命令尚未最终提交时的安全替换事实。
#[derive(Debug, Clone)]
pub struct AdminRuntimeReplacement {
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

/// runtime 命令账本安全投影。
#[derive(Debug, Clone)]
pub struct AdminRuntimeStoredCommand {
    pub command_id: CommandId,
    pub audit_id: Uuid,
    pub digest_version: u16,
    pub request_hash: RequestHash,
    pub expected_fingerprint: String,
    pub old_fingerprint: String,
    pub new_fingerprint: String,
    pub candidate_document: AdminRuntimeDocument,
    pub result_document: Option<AdminRuntimeDocument>,
    pub restart_required: bool,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub state: AdminRuntimeCommandState,
    pub unknown_category: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

impl AdminRuntimeStoredCommand {
    /// 校验账本行能否重建可信的候选和结果。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        if self.command_id.is_nil()
            || self.audit_id.is_nil()
            || self.digest_version != ADMIN_RUNTIME_DIGEST_VERSION
            || self.request_hash.as_bytes() == [0; 32]
            || !super::validation::valid_fingerprint(&self.expected_fingerprint)
            || self.expected_fingerprint != self.old_fingerprint
            || !super::validation::valid_fingerprint(&self.new_fingerprint)
            || !super::validation::valid_text(&self.actor_id, 256)
            || self
                .request_id
                .as_deref()
                .is_some_and(|value| !super::validation::valid_text(value, 256))
            || self.occurred_at.timestamp_micros() == 0
            || self.created_at.timestamp_micros() == 0
            || self.updated_at < self.created_at
            || !self.restart_required
            || self.candidate_document.validate().is_err()
            || self.candidate_document.fingerprint() != self.new_fingerprint
            || self.candidate_document.request_hash(&self.expected_fingerprint).ok()
                != Some(self.request_hash)
        {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt);
        }
        match self.state {
            AdminRuntimeCommandState::Pending => {
                if self.result_document.is_some()
                    || self.applied_at.is_some()
                    || self.unknown_category.is_some()
                {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                }
            }
            AdminRuntimeCommandState::Unknown => {
                if self.result_document.is_some()
                    || self.applied_at.is_some()
                    || !self
                        .unknown_category
                        .as_deref()
                        .is_some_and(super::validation::valid_unknown_category)
                {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                }
            }
            AdminRuntimeCommandState::Applied => {
                let Some(result) = &self.result_document else {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                };
                if self.applied_at.is_none()
                    || self.unknown_category.is_some()
                    || result.validate().is_err()
                    || result.fingerprint() != self.new_fingerprint
                    || result != &self.candidate_document
                {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                }
            }
        }
        Ok(())
    }

    pub fn matches_request(&self, request_hash: RequestHash) -> bool {
        self.digest_version == ADMIN_RUNTIME_DIGEST_VERSION && self.request_hash == request_hash
    }
}

/// 数据库审计写入所需的安全事实。
#[derive(Debug, Clone)]
pub struct AdminRuntimeAuditRecord {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub command_id: CommandId,
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

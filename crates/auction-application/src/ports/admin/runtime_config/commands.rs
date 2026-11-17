//! runtime 更新命令、命令账本状态与审计记录。

use auction_domain::CommandId;
use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    document::{
        ADMIN_RUNTIME_DIGEST_VERSION, ADMIN_RUNTIME_LEGACY_DIGEST_VERSION, AdminRuntimeDocument,
    },
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
    pub occurred_at: Timestamp,
    pub state: AdminRuntimeCommandState,
    pub unknown_category: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub applied_at: Option<Timestamp>,
}

impl AdminRuntimeStoredCommand {
    /// 校验账本行能否重建可信的候选和结果。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        if self.command_id.is_nil()
            || self.audit_id.is_nil()
            || !self.request_digest_binds_candidate()
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

    /// 存储层状态迁移（applied、unknown、重开）核对行身份：版本可识别，且摘要等于调用方持有的
    /// 本行摘要。
    pub fn matches_request(&self, request_hash: RequestHash) -> bool {
        matches!(
            self.digest_version,
            ADMIN_RUNTIME_DIGEST_VERSION | ADMIN_RUNTIME_LEGACY_DIGEST_VERSION
        ) && self.request_hash == request_hash
    }

    /// 同一 `command_id` 的重试是否就是本行记录的请求（不变量 1：同 ID 同请求返回原结果，同 ID
    /// 不同请求冲突）。v2 行只比较原始请求摘要，不再用本行候选补回请求省略的字段，因此省略与
    /// 显式写出（含显式写出默认值）互不相同。v1 旧行没有原始请求摘要，沿用原有口径：请求省略的
    /// 兼容字段以本行候选补回后计算 v1 摘要再比较，升级不会把旧命令的原样重试判为冲突。调用方
    /// 须先以 [`AdminRuntimeUpdateCommand::validate`] 核对请求摘要。
    pub fn replays(
        &self,
        command: &AdminRuntimeUpdateCommand,
    ) -> Result<bool, AdminRuntimeConfigError> {
        match self.digest_version {
            ADMIN_RUNTIME_DIGEST_VERSION => Ok(self.request_hash == command.request_hash),
            ADMIN_RUNTIME_LEGACY_DIGEST_VERSION => {
                let replayed = command.document.clone().resolve_update(&self.candidate_document);
                Ok(replayed.legacy_request_hash(&command.expected_fingerprint)?
                    == self.request_hash)
            }
            _ => Err(AdminRuntimeConfigError::StoredResultCorrupt),
        }
    }

    /// 摘要与候选的绑定按版本核对：v1 摘要按落成的候选计算，必须能由候选重算；v2 摘要只代表
    /// 原始请求，无法也无需由候选重算，候选由 `new_fingerprint` 绑定。未知版本失败关闭。
    fn request_digest_binds_candidate(&self) -> bool {
        match self.digest_version {
            ADMIN_RUNTIME_DIGEST_VERSION => true,
            ADMIN_RUNTIME_LEGACY_DIGEST_VERSION => {
                self.candidate_document.legacy_request_hash(&self.expected_fingerprint).ok()
                    == Some(self.request_hash)
            }
            _ => false,
        }
    }
}

/// 数据库审计写入所需的安全事实。
#[derive(Debug, Clone)]
pub struct AdminRuntimeAuditRecord {
    pub id: Uuid,
    pub occurred_at: Timestamp,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub command_id: CommandId,
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

#[cfg(test)]
mod tests {
    use auction_domain::CommandId;
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{AdminRuntimeCommandState, AdminRuntimeStoredCommand};
    use crate::ports::admin::runtime_config::document::{
        ADMIN_RUNTIME_DIGEST_VERSION, ADMIN_RUNTIME_LEGACY_DIGEST_VERSION, AdminRuntimeDocument,
    };
    use crate::ports::core::RequestHash;

    /// 摘要按候选以版本 1 编码计算的 pending 行。
    fn row(digest_version: u16) -> AdminRuntimeStoredCommand {
        let mut candidate = AdminRuntimeDocument::default();
        candidate.materialize_business_defaults();
        let expected = format!("sha256:{}", "0".repeat(64));
        let at = Timestamp::from_ymd_hms(2026, 11, 1, 0, 0, 0).expect("固定时刻");
        AdminRuntimeStoredCommand {
            command_id: CommandId::from_uuid(Uuid::from_u128(1)),
            audit_id: Uuid::from_u128(2),
            digest_version,
            request_hash: candidate.legacy_request_hash(&expected).expect("旧版摘要"),
            expected_fingerprint: expected.clone(),
            old_fingerprint: expected,
            new_fingerprint: candidate.fingerprint(),
            candidate_document: candidate,
            result_document: None,
            restart_required: true,
            actor_id: "admin".to_owned(),
            request_id: None,
            occurred_at: at,
            state: AdminRuntimeCommandState::Pending,
            unknown_category: None,
            created_at: at,
            updated_at: at,
            applied_at: None,
        }
    }

    /// 摘要与候选的绑定随版本：版本 1 的摘要必须能由候选重算，被改写即账本损坏；版本 2 的摘要
    /// 代表原始请求，不要求等于候选的摘要；未知版本失败关闭。
    #[test]
    fn stored_digest_binding_follows_the_digest_version() {
        assert!(row(ADMIN_RUNTIME_LEGACY_DIGEST_VERSION).validate().is_ok());
        let mut tampered = row(ADMIN_RUNTIME_LEGACY_DIGEST_VERSION);
        tampered.request_hash = RequestHash::new([7; 32]);
        assert!(tampered.validate().is_err(), "版本 1 摘要与候选不符即损坏");

        let mut request_identity = row(ADMIN_RUNTIME_DIGEST_VERSION);
        request_identity.request_hash = RequestHash::new([7; 32]);
        assert!(request_identity.validate().is_ok(), "版本 2 摘要只代表原始请求");
        assert!(request_identity.matches_request(RequestHash::new([7; 32])));

        let unknown = row(3);
        assert!(unknown.validate().is_err(), "未知摘要版本失败关闭");
        assert!(!unknown.matches_request(unknown.request_hash));
    }
}

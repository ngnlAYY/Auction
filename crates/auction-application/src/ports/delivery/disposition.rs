// 统一投递处置事实的端口与操作者确认命令（W2）。
//
// 处置事实只由存储层单一模块追加；本端口不定义第二套状态机，也不改写投递状态。
// 读侧只按 `resolves_alert` 过滤“未处置”，不再按来源拆开排除谓词。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApplicationFuture, RequestHash};

/// 投递处置的安全只读投影；不含 payload、凭据或原始证据正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryDispositionRecord {
    pub delivery_id: Uuid,
    pub fencing_version: i64,
    /// `DeliveryDispositionKind` 的稳定名称。
    pub kind: String,
    pub reason_code: Option<String>,
    /// `admin`、`cli`、`worker` 或 `system`。
    pub actor_source: String,
    pub actor_id: String,
    pub reason: Option<String>,
    /// 是否解除该 fence 上的失败计数（写入时固定的事实列）。
    pub resolves_alert: bool,
    pub occurred_at: DateTime<Utc>,
}

/// 操作者确认历史失败已处置；只适用于没有资金与资产影响的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAcknowledgementCommand {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    /// `admin` 或 `cli`；Worker 与 system 不得代替操作者确认。
    pub actor_source: auction_domain::DeliveryDispositionActorSource,
    pub actor_id: String,
    /// 处置原因；必填，不含凭据或正文原文。
    pub reason: String,
    /// CLI 系统身份（W9.7）；不进入幂等摘要，Admin 调用为 `None`。
    pub operator_identity: Option<crate::CliOperatorIdentity>,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl DeliveryAcknowledgementCommand {
    /// 同 ID 同参重放的稳定摘要；`request_id` 有意不进入摘要。
    pub fn request_hash(
        delivery_id: Uuid,
        expected_fencing_version: i64,
        actor_id: &str,
        reason: &str,
    ) -> RequestHash {
        let mut payload = Vec::with_capacity(256);
        payload.extend_from_slice(b"{\"delivery_id\":\"");
        payload.extend_from_slice(delivery_id.as_hyphenated().to_string().as_bytes());
        payload.extend_from_slice(b"\",\"expected_fencing_version\":");
        payload.extend_from_slice(expected_fencing_version.to_string().as_bytes());
        payload.extend_from_slice(b",\"kind\":\"operator_acknowledged\",\"actor_id\":\"");
        payload.extend_from_slice(actor_id.as_bytes());
        payload.extend_from_slice(b"\",\"reason\":");
        payload.extend_from_slice(reason.len().to_string().as_bytes());
        payload.push(b':');
        payload.extend_from_slice(reason.as_bytes());
        payload.push(b'}');
        RequestHash::from_canonical_bytes(&payload)
    }

    /// 命令形状校验；资格仍由最终事务重新读取，不能把已校验形状当作处置依据。
    pub fn validate(&self) -> Result<(), DeliveryDispositionCommandError> {
        if self.command_id.is_nil()
            || self.delivery_id.is_nil()
            || self.expected_fencing_version <= 0
            || !matches!(
                self.actor_source,
                auction_domain::DeliveryDispositionActorSource::Admin
                    | auction_domain::DeliveryDispositionActorSource::Cli
            )
            || (self.actor_source == auction_domain::DeliveryDispositionActorSource::Cli)
                != self.operator_identity.is_some()
            || auction_domain::validate_disposition_actor(&self.actor_id).is_err()
            || auction_domain::validate_disposition_reason(
                auction_domain::DeliveryDispositionKind::OperatorAcknowledged,
                Some(&self.reason),
            )
            .is_err()
            || self
                .request_id
                .as_ref()
                .is_some_and(|value| value.is_empty() || value != value.trim())
            || self.request_hash.as_bytes() == [0; 32]
        {
            return Err(DeliveryDispositionCommandError::Invalid);
        }
        Ok(())
    }
}

/// 原命令结果永久由处置事实行保存，同 ID 同参重放不重复追加。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAcknowledgementResult {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub fencing_version: i64,
    pub kind: String,
    pub replayed: bool,
}

/// 统一处置事实的读侧与操作者确认端口。
pub trait DeliveryDispositionPort: Send + Sync {
    /// 按发生时间倒序返回该投递的处置事实；不存在时返回空列表。
    fn dispositions(
        &self,
        delivery_id: Uuid,
    ) -> ApplicationFuture<'_, Vec<DeliveryDispositionRecord>>;

    /// 追加“操作者确认已处置”事实；同 ID 同参重放返回原结果。
    fn acknowledge(
        &self,
        command: DeliveryAcknowledgementCommand,
    ) -> ApplicationFuture<'_, DeliveryAcknowledgementResult>;
}

/// 操作者确认的稳定业务错误；只携带闭集分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryDispositionCommandError {
    #[error("投递处置命令无效")]
    Invalid,
    #[error("投递不存在")]
    NotFound,
    #[error("投递已被其他命令推进")]
    Stale,
    #[error("投递当前 fence 已有处置事实")]
    AlreadyDisposed,
    #[error("该投递没有确认已处置的资格")]
    NotAcknowledgeable,
    #[error("同一命令 ID 已用于不同请求")]
    CommandConflict,
    #[error("投递处置暂时不可用")]
    Unavailable,
}

impl From<auction_domain::DeliveryDispositionError> for DeliveryDispositionCommandError {
    fn from(error: auction_domain::DeliveryDispositionError) -> Self {
        match error {
            auction_domain::DeliveryDispositionError::NotAcknowledgeable => {
                Self::NotAcknowledgeable
            }
            auction_domain::DeliveryDispositionError::UnknownKind
            | auction_domain::DeliveryDispositionError::InvalidSource
            | auction_domain::DeliveryDispositionError::InvalidActor
            | auction_domain::DeliveryDispositionError::InvalidReason => Self::Invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::DeliveryAcknowledgementCommand;

    fn command() -> DeliveryAcknowledgementCommand {
        let delivery_id = Uuid::from_u128(7);
        let actor_id = "operator".to_owned();
        let reason = "历史的通知失败已核对，无资金或资产影响".to_owned();
        let hash = DeliveryAcknowledgementCommand::request_hash(delivery_id, 3, &actor_id, &reason);
        DeliveryAcknowledgementCommand {
            command_id: Uuid::from_u128(9),
            delivery_id,
            expected_fencing_version: 3,
            actor_source: auction_domain::DeliveryDispositionActorSource::Cli,
            actor_id,
            reason,
            operator_identity: Some(crate::CliOperatorIdentity {
                effective_uid: 1000,
                sudo_user: None,
            }),
            request_id: Some("req-1".to_owned()),
            request_hash: hash,
        }
    }

    #[test]
    fn accepts_complete_operator_command() {
        assert!(command().validate().is_ok());
    }

    #[test]
    fn rejects_worker_or_missing_reason() {
        let mut worker = command();
        worker.actor_source = auction_domain::DeliveryDispositionActorSource::Worker;
        assert!(worker.validate().is_err());

        let mut blank = command();
        blank.reason = "   ".to_owned();
        assert!(blank.validate().is_err());

        let mut stale = command();
        stale.expected_fencing_version = 0;
        assert!(stale.validate().is_err());
    }

    #[test]
    fn request_hash_is_stable_and_parameter_bound() {
        let base = command();
        let same = DeliveryAcknowledgementCommand::request_hash(
            base.delivery_id,
            base.expected_fencing_version,
            &base.actor_id,
            &base.reason,
        );
        assert_eq!(base.request_hash, same);
        assert_ne!(
            base.request_hash,
            DeliveryAcknowledgementCommand::request_hash(
                base.delivery_id,
                base.expected_fencing_version + 1,
                &base.actor_id,
                &base.reason,
            )
        );
        assert_ne!(
            base.request_hash,
            DeliveryAcknowledgementCommand::request_hash(
                base.delivery_id,
                base.expected_fencing_version,
                &base.actor_id,
                "另一条原因",
            )
        );
    }
}

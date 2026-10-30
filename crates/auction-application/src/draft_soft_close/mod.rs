//! 未发布草稿软截止修正：显式命令、资格规则和事务窄端口。
//! 只允许 60/60 秒旧快照改为 900/900 秒；不重算价格、排期、投稿或资金。
//! 存储必须先检查命令账本，再在同一 Serializable 事务中读取证据、调用规则并追加审计。

use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::RequestHash;

/// 草稿修正命令的稳定账本分类。
pub const DRAFT_SOFT_CLOSE_COMMAND_KIND: &str = "repair_draft_soft_close";
/// 本次显式修正的固定目标，不跟随之后的程序默认值漂移。
pub const REPAIRED_SOFT_CLOSE_SECONDS: i64 = 900;

/// 必须由操作者保留并原样重试的写入请求。
#[derive(Debug, Clone, Serialize)]
pub struct RepairDraftSoftCloseCommand {
    pub auction_id: Uuid,
    pub expected_version: i64,
    pub command_id: Uuid,
    pub actor: String,
    pub confirmation: String,
}

impl RepairDraftSoftCloseCommand {
    /// 校验精确确认文本和稳定身份；确认文本也纳入摘要。
    pub fn validate(&self) -> Result<(), DraftSoftCloseError> {
        if self.auction_id.is_nil()
            || self.command_id.is_nil()
            || self.expected_version <= 0
            || self.expected_version == i64::MAX
            || !auction_domain::valid_text(&self.actor, 128)
            || self.confirmation != format!("REPAIR DRAFT SOFT CLOSE {}", self.auction_id)
        {
            return Err(DraftSoftCloseError::InvalidRequest);
        }
        Ok(())
    }

    /// 摘要覆盖目标、版本、操作者及确认文本，异负载绝不重放。
    pub fn request_hash(&self) -> Result<RequestHash, DraftSoftCloseError> {
        self.validate()?;
        RequestHash::from_request(DRAFT_SOFT_CLOSE_COMMAND_KIND, self.auction_id, self)
            .map_err(|_| DraftSoftCloseError::InvalidRequest)
    }
}

/// 锁内事实，不包含数据库行或外部客户端。
#[derive(Debug)]
pub struct DraftSoftCloseSnapshot {
    pub auction_id: Uuid,
    pub state_version: i64,
    pub status: String,
    pub window_seconds: i64,
    pub extension_seconds: i64,
    pub default_start_price: i64,
    pub proxy_slow_interval_seconds: i64,
    pub policy_snapshot: Value,
    pub lifecycle_evidence: bool,
    pub publication_evidence: bool,
    pub competition_evidence: bool,
    pub settlement_evidence: bool,
    pub round_in_progress: bool,
}

impl DraftSoftCloseSnapshot {
    /// 只读和执行共用的失败关闭规则；草稿可保留尚未公开的投稿与 pending 装备。
    pub fn inspect(&self) -> DraftSoftCloseInspection {
        let mut blockers = Vec::new();
        if self.status != "draft" || self.lifecycle_evidence {
            blockers.push("not_pristine_draft".to_owned());
        }
        if self.publication_evidence {
            blockers.push("publication_or_unknown_evidence".to_owned());
        }
        if self.competition_evidence {
            blockers.push("competition_evidence".to_owned());
        }
        if self.settlement_evidence {
            blockers.push("settlement_evidence".to_owned());
        }
        if self.round_in_progress {
            blockers.push("scheduled_round_in_progress".to_owned());
        }
        if self.state_version <= 0 || self.state_version == i64::MAX {
            blockers.push("invalid_state_version".to_owned());
        }
        let policy = &self.policy_snapshot;
        if policy.get("schema_version").and_then(Value::as_i64) != Some(4)
            || policy.get("start_price").and_then(Value::as_i64) != Some(self.default_start_price)
            || policy.get("proxy_slow_interval_seconds").and_then(Value::as_i64)
                != Some(self.proxy_slow_interval_seconds)
            || policy.get("soft_close_window_seconds").and_then(Value::as_i64)
                != Some(self.window_seconds)
            || policy.get("soft_close_extension_seconds").and_then(Value::as_i64)
                != Some(self.extension_seconds)
        {
            blockers.push("policy_row_mismatch".to_owned());
        }
        if self.window_seconds != 60 || self.extension_seconds != 60 {
            blockers.push("not_legacy_60_60".to_owned());
        }
        DraftSoftCloseInspection {
            auction_id: self.auction_id,
            state_version: self.state_version,
            window_seconds: self.window_seconds,
            extension_seconds: self.extension_seconds,
            target_seconds: REPAIRED_SOFT_CLOSE_SECONDS,
            eligible: blockers.is_empty(),
            blockers,
        }
    }

    /// 账本未命中时才应用当前状态门禁，避免已成功命令因 900/900 新状态被拒绝。
    pub fn authorize(
        &self,
        command: &RepairDraftSoftCloseCommand,
    ) -> Result<(), DraftSoftCloseError> {
        command.validate()?;
        if self.auction_id != command.auction_id || self.state_version != command.expected_version {
            return Err(DraftSoftCloseError::Conflict);
        }
        if !self.inspect().eligible {
            return Err(DraftSoftCloseError::Ineligible);
        }
        Ok(())
    }
}

/// 默认只读检查结果；不会占用 command_id 或生成账本。
#[derive(Debug, Serialize)]
pub struct DraftSoftCloseInspection {
    pub auction_id: Uuid,
    pub state_version: i64,
    pub window_seconds: i64,
    pub extension_seconds: i64,
    pub target_seconds: i64,
    pub eligible: bool,
    pub blockers: Vec<String>,
}

/// 与变更、命令账本、审计同事务持久化的原始结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSoftCloseRepairResult {
    pub auction_id: Uuid,
    pub command_id: Uuid,
    pub previous_version: i64,
    pub state_version: i64,
    pub window_seconds: i64,
    pub extension_seconds: i64,
    pub repaired_at: DateTime<Utc>,
}

/// 显式运维修正错误；存储错误不暴露 SQL 或连接秘密。
#[derive(Debug, thiserror::Error)]
pub enum DraftSoftCloseError {
    #[error("草稿修正参数或精确确认文本无效")]
    InvalidRequest,
    #[error("目标拍卖不存在")]
    NotFound,
    #[error("版本或命令负载冲突")]
    Conflict,
    #[error("目标不符合从未发布草稿且规则一致为60/60的门禁；请先只读检查")]
    Ineligible,
    #[error("草稿修正存储操作失败")]
    Storage,
    #[error("草稿修正串行化冲突达到三次上限；请保留原命令重试")]
    RetryExhausted,
}

pub type DraftSoftCloseFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DraftSoftCloseError>> + Send + 'a>>;

/// 存储拥有 SQL/锁/重试；业务资格由上述快照规则唯一决定。
pub trait DraftSoftClosePort: Send + Sync {
    fn inspect_draft_soft_close(
        &self,
        auction_id: Uuid,
    ) -> DraftSoftCloseFuture<'_, DraftSoftCloseInspection>;
    fn repair_draft_soft_close(
        &self,
        command: RepairDraftSoftCloseCommand,
    ) -> DraftSoftCloseFuture<'_, DraftSoftCloseRepairResult>;
}

//! 恢复后外部核对的本地事实与隔离写入端口。
//!
//! 远端 Sent / Read Mail / 论坛读取只经 Worker control 执行；本模块只拥有恢复库中的
//! 投递发送预期、已知入站邮件、已发布论坛主题游标，以及“与 Sent 精确匹配”后的隔离写入。
//!
//! 不变量：
//! - 隔离只改变 pending/claimed 投递的状态与 fence，不重发、不领取、不改写资金或结算事实；
//! - 匹配必须同时成立：世界、Sent 详情完整收件人、发送时间不早于恢复点；冻结发送身份另需
//!   标题一致，资产仍需与恢复库身份一致，已领取邮件只能按完整身份匹配；
//! - 恢复库没有该投递当前 fence 的冻结尝试时，只能用 payload 重建身份：没有可证明的标题，
//!   结论是未决而不是精确证明，且无法唯一归属 Sent 时必须显式阻塞而不是继续可领取；
//! - 列表投影不是身份证明，只有详情读取后的完整字段才可产生隔离；
//! - 同一投递与 fence 的隔离事实只追加一次；重复执行幂等返回既有结果，不新增审计。

use std::future::Future;
use std::pin::Pin;

use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::ApplicationError;

/// 恢复核对回执格式；CLI 输出文件与文档共用。
pub const EXTERNAL_RECOVERY_RECEIPT_FORMAT: &str = "auction-external-recovery.v1";
/// 单轮读取的投递预期上限；超过时只处理前 N 条并在回执标记截断。
pub const MAX_EXTERNAL_RECOVERY_DELIVERIES: usize = 1_000;
/// 单轮论坛主题上限；与 Worker control 请求上限一致。
pub const MAX_EXTERNAL_RECOVERY_FORUM_THREADS: u16 = 20;
/// 未决阻塞回执里单条投递最多保留的候选 MID 数；超出只影响回执细节，不改变阻塞。
pub const MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS: usize = 16;
/// 无法唯一归属 Sent 时的稳定阻塞原因码。
pub const EXTERNAL_RECOVERY_AMBIGUOUS_REASON: &str = "ambiguous_sent_candidates";

pub type ExternalRecoveryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// 恢复库中投递发送身份的来源。
///
/// 恢复库可能没有该投递当前 fence 的冻结发送尝试：投递在备份点仍是 `pending`/`claimed`，
/// 旧 Worker 在快照之后才进入 `sending`，而 `delivery_send_attempts` 与 `sending` 状态同事务
/// 写入，因此这份身份不在恢复范围内。此时只能用恢复库自身的 payload 重建世界、收件人与资产，
/// 没有可证明的渲染标题与正文摘要，匹配强度更低，只能给出“未决”结论而不是精确证明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalRecoverySendEvidence {
    /// sending fence 冻结的完整发送预期，含渲染标题与资产身份。
    Frozen,
    /// 从恢复库 payload 重建的世界/收件人/资产身份，没有渲染标题。
    RestoredPayload,
}

/// 恢复库中仍可被备份后 Sent 影响的投递发送身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRecoveryDeliveryExpectation {
    pub delivery_id: Uuid,
    pub fencing_version: i64,
    pub evidence: ExternalRecoverySendEvidence,
    pub world_id: String,
    pub recipient: String,
    /// 只有冻结预期能证明渲染标题；payload 重建时为 `None`。
    pub subject: Option<String>,
    pub credits: i64,
    pub cod: i64,
    /// `None` 表示恢复库 payload 无法完整重建装备身份（未知，不是“无附件”）。
    pub equipment_ids: Option<Vec<String>>,
}

impl ExternalRecoveryDeliveryExpectation {
    pub fn validate(&self) -> bool {
        let evidence_shape = match self.evidence {
            ExternalRecoverySendEvidence::Frozen => {
                self.fencing_version > 0 && self.subject.is_some() && self.equipment_ids.is_some()
            }
            // 未领取的 pending 投递默认 fence=0；这正是没有冻结发送尝试的恢复场景。
            ExternalRecoverySendEvidence::RestoredPayload => {
                self.fencing_version >= 0 && self.subject.is_none()
            }
        };
        !self.delivery_id.is_nil()
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && self.subject.as_deref().is_none_or(|subject| valid_text(subject, 512))
            && evidence_shape
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.credits)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.cod)
            && self.equipment_ids.as_ref().is_none_or(|ids| {
                ids.len() <= 1_000
                    && ids.iter().all(|id| crate::ports::settlement::valid_external_positive_id(id))
                    && ids.windows(2).all(|ids| ids[0] < ids[1])
            })
    }
}

/// 恢复库已发布的论坛主题与其已处理楼层；远端之后的新楼层不属于恢复库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRecoveryForumThread {
    pub auction_id: Uuid,
    pub world_id: String,
    pub thread_id: String,
    /// 恢复库已处理到的最高楼层；新帖必须严格大于该值。
    pub after_post_number: u32,
}

impl ExternalRecoveryForumThread {
    pub fn validate(&self) -> bool {
        !self.auction_id.is_nil()
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && auction_domain::valid_positive_decimal(&self.thread_id, 32)
            && self.after_post_number >= 1
    }
}

/// 与 Sent 详情精确匹配的候选；写入前在事务内再次核对当前投递与发送预期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRecoveryQuarantineMatch {
    pub delivery_id: Uuid,
    /// 匹配时读取到的 fence；事务内必须仍是该值。
    pub fencing_version: i64,
    pub world_id: String,
    pub external_mail_id: String,
    pub sent_at: DateTime<Utc>,
    pub recipient: String,
    pub subject: String,
    pub expected_credits: i64,
    pub expected_cod: i64,
    /// `None` 表示恢复库无法重建装备身份；隔离仍按已知字段重新核对。
    pub expected_equipment_ids: Option<Vec<String>>,
    /// Sent 详情是否证明邮件已被完全领取；为 true 时不再要求剩余资产一致。
    pub collected: bool,
    pub observed_at: DateTime<Utc>,
}

impl ExternalRecoveryQuarantineMatch {
    pub fn validate(&self) -> bool {
        // 隔离证据表要求 fence >=1；pending fence=0 的匹配由 CLI 路由到未决阻塞。
        !self.delivery_id.is_nil()
            && self.fencing_version > 0
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && auction_domain::valid_positive_decimal(&self.external_mail_id, 64)
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && valid_text(&self.subject, 512)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.expected_credits)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.expected_cod)
            && self.expected_equipment_ids.as_ref().is_none_or(|ids| {
                ids.len() <= 1_000
                    && ids.iter().all(|id| crate::ports::settlement::valid_external_positive_id(id))
            })
            && self.sent_at.timestamp_nanos_opt().is_some()
            && self.observed_at.timestamp_nanos_opt().is_some()
    }
}

/// 无法唯一归属 Sent、必须显式阻塞为人工对账的恢复投递。
///
/// 只携带本地投递身份与观察到的候选 MID；不构成发送、领取或收款证明。不写入
/// `delivery_external_quarantines`（同一 MID 只能证明一条投递）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRecoveryUnresolvedBlock {
    pub delivery_id: Uuid,
    /// 读取候选时看到的 fence；事务内必须仍是该值。
    pub fencing_version: i64,
    /// 闭集；目前只有 `ambiguous_sent_candidates`。
    pub reason: String,
    /// 观察到的候选 MID；只用于回执与审计细节。
    pub candidate_mail_ids: Vec<String>,
}

impl ExternalRecoveryUnresolvedBlock {
    pub fn validate(&self) -> bool {
        !self.delivery_id.is_nil()
            && self.fencing_version >= 0
            && self.reason == EXTERNAL_RECOVERY_AMBIGUOUS_REASON
            && self.candidate_mail_ids.len() <= MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS
            && self
                .candidate_mail_ids
                .iter()
                .all(|id| auction_domain::valid_positive_decimal(id, 64))
    }
}

/// 单条隔离结果；`status` 闭集为 `quarantined`、`blocked` 或 `skipped`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalRecoveryQuarantineOutcome {
    pub delivery_id: Uuid,
    pub status: String,
    pub fencing_version: Option<i64>,
    pub reason: Option<String>,
}

impl ExternalRecoveryQuarantineOutcome {
    pub fn quarantined(delivery_id: Uuid, fencing_version: i64) -> Self {
        Self {
            delivery_id,
            status: "quarantined".to_owned(),
            fencing_version: Some(fencing_version),
            reason: None,
        }
    }

    /// 没有唯一外部证据、按本地未决阻塞人工对账。
    pub fn blocked(delivery_id: Uuid, fencing_version: i64) -> Self {
        Self {
            delivery_id,
            status: "blocked".to_owned(),
            fencing_version: Some(fencing_version),
            reason: Some(EXTERNAL_RECOVERY_AMBIGUOUS_REASON.to_owned()),
        }
    }

    pub fn skipped(delivery_id: Uuid, reason: &str) -> Self {
        Self {
            delivery_id,
            status: "skipped".to_owned(),
            fencing_version: None,
            reason: Some(reason.to_owned()),
        }
    }
}

pub trait ExternalRecoveryStore: Send + Sync {
    /// 读取 pending/claimed 的 Send 投递身份，按创建顺序。
    /// 有当前 fence 冻结发送预期时给出完整身份；否则用恢复库 payload 重建（证据更弱）。
    fn pending_claimed_send_expectations(
        &self,
    ) -> ExternalRecoveryFuture<'_, Vec<ExternalRecoveryDeliveryExpectation>>;

    /// 从候选 `(world, external_mail_id)` 中筛出恢复库已有记录的项。
    /// 只读查询，不展开为全表枚举。
    fn known_inbound_mail_ids(
        &self,
        candidates: Vec<(String, String)>,
    ) -> ExternalRecoveryFuture<'_, Vec<(String, String)>>;

    /// 已发布论坛主题及其已处理楼层，按最近同步倒序。
    fn published_forum_threads(
        &self,
        limit: u16,
    ) -> ExternalRecoveryFuture<'_, Vec<ExternalRecoveryForumThread>>;

    /// 把与 Sent 精确匹配的 pending/claimed 投递隔离为 manual_reconcile。
    /// 每条匹配在事务内重新锁定并核对 fence 与发送身份；重复扫描幂等。
    fn quarantine_external_sent_matches(
        &self,
        scan_id: Uuid,
        actor_id: String,
        matches: Vec<ExternalRecoveryQuarantineMatch>,
    ) -> ExternalRecoveryFuture<'_, Vec<ExternalRecoveryQuarantineOutcome>>;

    /// 把无法唯一归属 Sent 的 pending/claimed 投递显式阻塞为 manual_reconcile。
    /// 只做本地状态收口并追加审计，不写外部证据、不重发、不领取。
    fn block_unresolved_send_deliveries(
        &self,
        scan_id: Uuid,
        actor_id: String,
        blocks: Vec<ExternalRecoveryUnresolvedBlock>,
    ) -> ExternalRecoveryFuture<'_, Vec<ExternalRecoveryQuarantineOutcome>>;
}

/// 恢复核对中的客户端匹配结果；用于回执与测试。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentDetailMatch {
    /// 完整身份与资产一致；可以进入隔离。
    Exact,
    /// 世界、收件人、标题一致但资产不再一致；未领取邮件视为不匹配。
    AssetMismatch,
    /// 完整身份不一致。
    IdentityMismatch,
    /// 候选不唯一；不得隔离。
    Ambiguous,
}

/// Sent 列表标题会被服务端按字节截断；只接受可证明的前缀。
pub fn subject_is_list_projection(list_subject: &str, detail_subject: &str) -> bool {
    if list_subject.is_empty() || detail_subject.is_empty() {
        return false;
    }
    if detail_subject.starts_with(list_subject) {
        return true;
    }
    let without_replacement = list_subject.trim_end_matches('\u{FFFD}');
    if !without_replacement.is_empty() && detail_subject.starts_with(without_replacement) {
        return true;
    }
    let without_ellipsis = without_replacement
        .strip_suffix('\u{2026}')
        .or_else(|| without_replacement.strip_suffix("..."));
    without_ellipsis.is_some_and(|prefix| !prefix.is_empty() && detail_subject.starts_with(prefix))
}

/// 列表行是否可能属于该投递；只用于筛选详情候选，不能单独产生隔离。
/// 冻结预期额外要求标题前缀投影；payload 重建没有可证明的标题，只能用世界+收件人+时间。
pub fn sent_list_projection_may_match(
    expectation: &ExternalRecoveryDeliveryExpectation,
    world: &str,
    recipient: &str,
    subject: &str,
    sent_at: DateTime<Utc>,
    cutoff_at: DateTime<Utc>,
) -> bool {
    expectation.world_id == world
        && sent_at >= cutoff_at
        && crate::ports::mail::mail_recipient_is_list_projection(recipient, &expectation.recipient)
        && expectation
            .subject
            .as_deref()
            .is_none_or(|expected| subject_is_list_projection(subject, expected))
}

/// 详情字段是否与该投递身份一致。
/// 冻结预期按完整身份（含标题）核对；payload 重建没有标题，只能按世界、收件人与资产核对，
/// 未知装备身份不会因此被当成不匹配。
/// 已领取邮件不再保留剩余资产：冻结预期仍靠标题身份；payload 重建无法再核对资产，
/// 只能给出未决结论，调用方不得把它当作已发送证明。
// 参数直接对应发送身份与单次 Sent 详情字段；拆成结构体不会减少校验点，反而增加临时投影。
#[allow(clippy::too_many_arguments)]
pub fn sent_detail_match(
    expectation: &ExternalRecoveryDeliveryExpectation,
    world: &str,
    recipient: &str,
    subject: &str,
    sent_at: DateTime<Utc>,
    cutoff_at: DateTime<Utc>,
    collected: bool,
    remaining_credits: i64,
    cod_pending: i64,
    remaining_equipment_ids: &[String],
) -> SentDetailMatch {
    if expectation.world_id != world
        || sent_at < cutoff_at
        || !recipient.eq_ignore_ascii_case(&expectation.recipient)
    {
        return SentDetailMatch::IdentityMismatch;
    }
    if let Some(expected_subject) = &expectation.subject
        && expected_subject != subject
    {
        return SentDetailMatch::IdentityMismatch;
    }
    if collected {
        // 剩余资产为零，无法再核对。冻结预期仍有完整标题身份；payload 重建只有世界+收件人，
        // 不足以把“已领取”归因到这条投递，必须交给调用方阻塞而不是隔离。
        return match expectation.evidence {
            ExternalRecoverySendEvidence::Frozen => SentDetailMatch::Exact,
            ExternalRecoverySendEvidence::RestoredPayload => SentDetailMatch::Ambiguous,
        };
    }
    if remaining_credits != expectation.credits || cod_pending != expectation.cod {
        return SentDetailMatch::AssetMismatch;
    }
    if let Some(expected_ids) = &expectation.equipment_ids
        && expected_ids != remaining_equipment_ids
    {
        return SentDetailMatch::AssetMismatch;
    }
    SentDetailMatch::Exact
}

#[cfg(test)]
mod tests;

// W10.9 恢复后外部核对的本地事实与隔离写入端口。
//
// 远端 Sent / Read Mail / 论坛读取只经 Worker control 执行；本模块只拥有恢复库中的
// 投递发送预期、已知入站邮件、已发布论坛主题游标，以及“与 Sent 精确匹配”后的隔离写入。
//
// 不变量：
// - 隔离只改变 pending/claimed 投递的状态与 fence，不重发、不领取、不改写资金或结算事实；
// - 匹配必须同时成立：世界、Sent 详情完整收件人与标题、发送时间不早于恢复点；资产仍需与
//   冻结发送预期一致，已领取邮件只能按完整身份+标题匹配；
// - 列表投影不是身份证明，只有详情读取后的完整字段才可产生隔离；
// - 同一投递与 fence 的隔离事实只追加一次；重复执行幂等返回既有结果，不新增审计。

use std::future::Future;
use std::pin::Pin;

use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::errors::ApplicationError;

/// 恢复核对回执格式；CLI 输出文件与文档共用。
pub const EXTERNAL_RECOVERY_RECEIPT_FORMAT: &str = "auction-external-recovery.v1";
/// 单轮读取的投递预期上限；超过时只处理前 N 条并在回执标记截断。
pub const MAX_EXTERNAL_RECOVERY_DELIVERIES: usize = 1_000;
/// 单轮论坛主题上限；与 Worker control 请求上限一致。
pub const MAX_EXTERNAL_RECOVERY_FORUM_THREADS: u16 = 20;
/// 单条投递最多接受的精确 Sent 详情候选；出现多个即视为不唯一。
pub const MAX_EXTERNAL_RECOVERY_SENT_MATCHES: usize = 2;

pub type ExternalRecoveryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// 恢复库中仍可被备份后 Sent 影响的投递发送预期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRecoveryDeliveryExpectation {
    pub delivery_id: Uuid,
    pub fencing_version: i64,
    pub world_id: String,
    pub recipient: String,
    pub subject: String,
    pub credits: i64,
    pub cod: i64,
    pub equipment_ids: Vec<String>,
    pub sending_at: DateTime<Utc>,
}

impl ExternalRecoveryDeliveryExpectation {
    pub fn validate(&self) -> bool {
        !self.delivery_id.is_nil()
            && self.fencing_version > 0
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && valid_text(&self.subject, 512)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.credits)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.cod)
            && self.equipment_ids.len() <= 1_000
            && self
                .equipment_ids
                .iter()
                .all(|id| crate::ports::settlement::valid_external_positive_id(id))
            && self.equipment_ids.windows(2).all(|ids| ids[0] < ids[1])
            && self.sending_at.timestamp_nanos_opt().is_some()
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
    pub expected_equipment_ids: Vec<String>,
    /// Sent 详情是否证明邮件已被完全领取；为 true 时不再要求剩余资产一致。
    pub collected: bool,
    pub observed_at: DateTime<Utc>,
}

impl ExternalRecoveryQuarantineMatch {
    pub fn validate(&self) -> bool {
        !self.delivery_id.is_nil()
            && self.fencing_version > 0
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && auction_domain::valid_positive_decimal(&self.external_mail_id, 64)
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && valid_text(&self.subject, 512)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.expected_credits)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.expected_cod)
            && self.expected_equipment_ids.len() <= 1_000
            && self
                .expected_equipment_ids
                .iter()
                .all(|id| crate::ports::settlement::valid_external_positive_id(id))
            && self.sent_at.timestamp_nanos_opt().is_some()
            && self.observed_at.timestamp_nanos_opt().is_some()
    }
}

/// 单条隔离结果；`status` 闭集为 `quarantined` 或 `skipped`。
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
    /// 读取 pending/claimed 且存在当前 fence 发送预期的投递，按发送时间升序。
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
    /// 每条匹配在事务内重新锁定并核对 fence 与发送预期；重复扫描幂等。
    fn quarantine_external_sent_matches(
        &self,
        scan_id: Uuid,
        actor_id: String,
        matches: Vec<ExternalRecoveryQuarantineMatch>,
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

/// 列表行是否可能属于该发送预期；只用于筛选详情候选，不能单独产生隔离。
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
        && subject_is_list_projection(subject, &expectation.subject)
}

/// 详情字段是否与冻结发送预期完全一致。
/// 已领取邮件不再保留剩余资产，只能按完整身份与标题判断；标题含唯一业务引用，仍是强身份。
// 参数直接对应冻结发送预期与单次 Sent 详情字段；拆成结构体不会减少校验点，反而增加临时投影。
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
        || subject != expectation.subject
    {
        return SentDetailMatch::IdentityMismatch;
    }
    if collected
        || (remaining_credits == expectation.credits
            && cod_pending == expectation.cod
            && remaining_equipment_ids == expectation.equipment_ids)
    {
        SentDetailMatch::Exact
    } else {
        SentDetailMatch::AssetMismatch
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::recovery::external as parent;
    use chrono::TimeDelta;

    fn expectation() -> ExternalRecoveryDeliveryExpectation {
        ExternalRecoveryDeliveryExpectation {
            delivery_id: Uuid::from_u128(7),
            fencing_version: 3,
            world_id: "main".to_owned(),
            recipient: "Restia﹒Ashdoll".to_owned(),
            subject: "[Auction] settlement payment 11111111-2222-3333-4444-555555555555".to_owned(),
            credits: 0,
            cod: 0,
            equipment_ids: vec!["317632875".to_owned()],
            sending_at: Utc::now() - TimeDelta::minutes(30),
        }
    }

    #[test]
    fn subject_projection_accepts_truncation_only() {
        let expected = expectation().subject;
        assert!(subject_is_list_projection(&expected, &expected));
        assert!(subject_is_list_projection("[Auction] settlement pay", &expected));
        assert!(subject_is_list_projection("[Auction] settlement pay\u{2026}", &expected));
        assert!(!subject_is_list_projection("[Auction] other", &expected));
        assert!(!subject_is_list_projection("", &expected));
    }

    #[test]
    fn list_projection_requires_world_and_cutoff() {
        let expectation = expectation();
        let cutoff = Utc::now() - TimeDelta::hours(1);
        assert!(sent_list_projection_may_match(
            &expectation,
            "main",
            "Restia",
            "[Auction] settlement",
            Utc::now() - TimeDelta::minutes(10),
            cutoff,
        ));
        assert!(!sent_list_projection_may_match(
            &expectation,
            "isekai",
            "Restia",
            "[Auction] settlement",
            Utc::now() - TimeDelta::minutes(10),
            cutoff,
        ));
        assert!(!sent_list_projection_may_match(
            &expectation,
            "main",
            "Restia",
            "[Auction] settlement",
            cutoff - TimeDelta::seconds(1),
            cutoff,
        ));
    }

    #[test]
    fn detail_match_requires_full_identity_and_assets() {
        let expectation = expectation();
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let sent_at = Utc::now() - TimeDelta::minutes(10);
        assert_eq!(
            sent_detail_match(
                &expectation,
                "main",
                "restia﹒ashdoll",
                &expectation.subject,
                sent_at,
                cutoff,
                false,
                0,
                0,
                &["317632875".to_owned()],
            ),
            SentDetailMatch::Exact
        );
        // 已领取：剩余资产为零，只按完整身份与标题判断。
        assert_eq!(
            sent_detail_match(
                &expectation,
                "main",
                &expectation.recipient,
                &expectation.subject,
                sent_at,
                cutoff,
                true,
                0,
                0,
                &[],
            ),
            SentDetailMatch::Exact
        );
        // 未领取但资产不一致：不得隔离。
        assert_eq!(
            sent_detail_match(
                &expectation,
                "main",
                &expectation.recipient,
                &expectation.subject,
                sent_at,
                cutoff,
                false,
                0,
                0,
                &["317632876".to_owned()],
            ),
            SentDetailMatch::AssetMismatch
        );
        assert_eq!(
            sent_detail_match(
                &expectation,
                "main",
                "Other",
                &expectation.subject,
                sent_at,
                cutoff,
                false,
                0,
                0,
                &["317632875".to_owned()],
            ),
            SentDetailMatch::IdentityMismatch
        );
    }

    #[test]
    fn expectation_rejects_unsorted_equipment_and_non_world() {
        let mut expectation = expectation();
        expectation.equipment_ids = vec!["2".to_owned(), "1".to_owned()];
        assert!(!expectation.validate());
        expectation.equipment_ids = vec!["1".to_owned()];
        expectation.world_id = "shared".to_owned();
        assert!(!expectation.validate());
    }
}

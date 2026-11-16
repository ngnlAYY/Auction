//! Admin recovery 报告的安全投影与读取端口。

use auction_domain::{AuctionId, LotId, SettlementId};
use chrono::{DateTime, Utc};

use crate::ports::core::ApplicationFuture;
use crate::ports::recovery::commands::RecoveryConfirmationAction;

/// Admin recovery action 的安全投影。
/// 不包含 claim token、lease、request digest、邮件正文或外部 payload；这些字段只能留在
/// Worker/适配器边界，不能通过管理报告泄露或被浏览器回放。
#[derive(Debug, Clone)]
pub struct RecoveryActionReport {
    pub id: SettlementId,
    pub kind: String,
    pub state: String,
    pub delivery_id: Option<SettlementId>,
    pub failure_code: Option<String>,
    pub proof_kind: Option<String>,
    pub proof_observed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Admin recovery case 的安全投影。
#[derive(Debug, Clone)]
pub struct RecoveryReportEntry {
    pub id: SettlementId,
    pub auction_id: AuctionId,
    pub lot_id: LotId,
    pub lot_number: i32,
    pub world_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub stage: String,
    pub resolution: Option<String>,
    pub state_version: i64,
    pub payment_deadline_at: DateTime<Utc>,
    pub reminder_count: i32,
    pub reminder_limit: i32,
    /// 已计入的 COD 退回次数与冻结上限；决定确认是否须先完成买家拉黑。
    pub cod_return_count: i32,
    pub cod_return_limit: i32,
    pub custody_proof_kind: Option<String>,
    pub custody_proof_observed_at: Option<DateTime<Utc>>,
    pub blocking_code: Option<String>,
    pub manual_reconcile_delivery_id: Option<SettlementId>,
    pub relisted_auction_id: Option<AuctionId>,
    pub relisted_lot_id: Option<LotId>,
    pub actions: Vec<RecoveryActionReport>,
    pub allowed_commands: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

/// 一个原拍卖的一致 recovery 报告。
#[derive(Debug, Clone)]
pub struct RecoveryReport {
    pub auction_id: AuctionId,
    pub recoveries: Vec<RecoveryReportEntry>,
}

pub trait RecoveryReportReader: Send + Sync {
    fn recovery_report(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<RecoveryReport>>;
}

/// 确认动作是否要求先完成买家拉黑；确认按钮与确认事务共用这一条规则。
///
/// - `reclaimed`（main）：未达冻结上限的退回由系统自动重寄，只有达到上限后才可确认，始终要求
///   既定拉黑已完成；
/// - `cancelled`（isekai）：载体没有自动重寄，载体收回证明成立后即可由 Admin 收口；退回次数未达
///   冻结上限时不拉黑（不改变冻结的处罚阈值），达到上限仍要求既定拉黑已完成；
/// - `equipment_recovered`：买方已付款，不涉及拉黑。
pub fn recovery_confirmation_requires_blacklist(
    action: RecoveryConfirmationAction,
    cod_return_count: i32,
    cod_return_limit: i32,
) -> bool {
    match action {
        RecoveryConfirmationAction::Reclaimed => true,
        RecoveryConfirmationAction::Cancelled => cod_return_count >= cod_return_limit,
        RecoveryConfirmationAction::EquipmentRecovered => false,
    }
}

/// 从已持久化、已满足的 action 事实派生允许的确认动作。
/// 这段闭集规则只决定 Admin 可以看到哪些确认按钮，不会改变 recovery 状态；真正确认时
/// 仍需由确认命令的 Serializable 事务重新锁定并验证全部事实。
pub fn allowed_recovery_commands(entry: &RecoveryReportEntry) -> Vec<String> {
    if entry.stage != "reclaiming"
        || entry.blocking_code.is_some()
        || entry.custody_proof_kind.is_none()
        || entry.custody_proof_observed_at.is_none()
    {
        return Vec::new();
    }

    let mut blacklist_satisfied = false;
    let mut candidate = None;
    for action in &entry.actions {
        if action.state != "satisfied" {
            continue;
        }
        if action.kind == "buyer_blacklist" {
            blacklist_satisfied = true;
            continue;
        }
        let next = match (entry.world_id.as_str(), action.kind.as_str()) {
            ("main", "reclaim_equipment_take") => Some(RecoveryConfirmationAction::Reclaimed),
            ("isekai", "cancel_carrier_take") => Some(RecoveryConfirmationAction::Cancelled),
            ("isekai", "recover_equipment_take") => {
                Some(RecoveryConfirmationAction::EquipmentRecovered)
            }
            _ => None,
        };
        let Some(next) = next else {
            continue;
        };
        if candidate.is_some_and(|current| current != next) {
            return Vec::new();
        }
        candidate = Some(next);
    }

    match candidate {
        Some(action)
            if blacklist_satisfied
                || !recovery_confirmation_requires_blacklist(
                    action,
                    entry.cod_return_count,
                    entry.cod_return_limit,
                ) =>
        {
            vec![action.as_str().to_owned()]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::{AuctionId, LotId, SettlementId};
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{
        RecoveryActionReport, RecoveryReportEntry, allowed_recovery_commands,
        recovery_confirmation_requires_blacklist,
    };
    use crate::ports::recovery::commands::RecoveryConfirmationAction;

    fn action(kind: &str, state: &str) -> RecoveryActionReport {
        RecoveryActionReport {
            id: SettlementId::from_uuid(Uuid::new_v4()),
            kind: kind.to_owned(),
            state: state.to_owned(),
            delivery_id: None,
            failure_code: None,
            proof_kind: Some("delivery_recovery_take_succeeded".to_owned()),
            proof_observed_at: None,
            updated_at: Utc.with_ymd_and_hms(2026, 11, 5, 9, 0, 0).single().unwrap(),
        }
    }

    fn entry(
        world: &str,
        count: i32,
        limit: i32,
        actions: Vec<RecoveryActionReport>,
    ) -> RecoveryReportEntry {
        let at = Utc.with_ymd_and_hms(2026, 11, 5, 9, 0, 0).single().unwrap();
        RecoveryReportEntry {
            id: SettlementId::from_uuid(Uuid::from_u128(1)),
            auction_id: AuctionId::from_uuid(Uuid::from_u128(2)),
            lot_id: LotId::from_uuid(Uuid::from_u128(3)),
            lot_number: 1,
            world_id: world.to_owned(),
            equipment_name: "Fixture".to_owned(),
            equipment_category: "weapon".to_owned(),
            stage: "reclaiming".to_owned(),
            resolution: None,
            state_version: 4,
            payment_deadline_at: at,
            reminder_count: 0,
            reminder_limit: 1,
            cod_return_count: count,
            cod_return_limit: limit,
            custody_proof_kind: Some("delivery_recovery_take_succeeded".to_owned()),
            custody_proof_observed_at: Some(at),
            blocking_code: None,
            manual_reconcile_delivery_id: None,
            relisted_auction_id: None,
            relisted_lot_id: None,
            actions,
            allowed_commands: Vec::new(),
            updated_at: at,
        }
    }

    #[test]
    fn blacklist_requirement_depends_on_action_and_frozen_limit() {
        for (count, limit) in [(1, 1), (2, 2), (3, 3), (0, 1), (1, 3), (2, 3)] {
            assert!(recovery_confirmation_requires_blacklist(
                RecoveryConfirmationAction::Reclaimed,
                count,
                limit
            ));
            assert!(!recovery_confirmation_requires_blacklist(
                RecoveryConfirmationAction::EquipmentRecovered,
                count,
                limit
            ));
            assert_eq!(
                recovery_confirmation_requires_blacklist(
                    RecoveryConfirmationAction::Cancelled,
                    count,
                    limit
                ),
                count >= limit,
                "count={count} limit={limit}"
            );
        }
    }

    #[test]
    fn isekai_cancel_button_follows_the_same_rule() {
        let take = || vec![action("cancel_carrier_take", "satisfied")];
        assert_eq!(allowed_recovery_commands(&entry("isekai", 1, 3, take())), ["cancelled"]);
        assert_eq!(allowed_recovery_commands(&entry("isekai", 1, 2, take())), ["cancelled"]);
        assert!(allowed_recovery_commands(&entry("isekai", 1, 1, take())).is_empty());
        assert!(allowed_recovery_commands(&entry("isekai", 3, 3, take())).is_empty());
        let mut with_blacklist = take();
        with_blacklist.push(action("buyer_blacklist", "satisfied"));
        assert_eq!(
            allowed_recovery_commands(&entry("isekai", 3, 3, with_blacklist)),
            ["cancelled"]
        );
        // 收回证明未满足时仍不出现按钮。
        assert!(
            allowed_recovery_commands(&entry(
                "isekai",
                1,
                3,
                vec![action("cancel_carrier_take", "enqueued")]
            ))
            .is_empty()
        );
    }

    #[test]
    fn main_reclaim_and_equipment_recovery_keep_their_rules() {
        let reclaim = || vec![action("reclaim_equipment_take", "satisfied")];
        assert!(allowed_recovery_commands(&entry("main", 1, 3, reclaim())).is_empty());
        let mut with_blacklist = reclaim();
        with_blacklist.push(action("buyer_blacklist", "satisfied"));
        assert_eq!(allowed_recovery_commands(&entry("main", 3, 3, with_blacklist)), ["reclaimed"]);
        assert_eq!(
            allowed_recovery_commands(&entry(
                "isekai",
                1,
                3,
                vec![action("recover_equipment_take", "satisfied")]
            )),
            ["equipment_recovered"]
        );
    }
}

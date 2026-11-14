//! Admin recovery 报告的安全投影与读取端口。

use auction_domain::{AuctionId, LotId, SettlementId};
use chrono::{DateTime, Utc};

use crate::ports::auction::ApplicationFuture;

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

/// 从已持久化、已满足的 action 事实派生允许的确认动作。
/// 这段闭集规则只决定 Admin 可以看到哪些确认按钮，不会改变 recovery 状态；真正确认时
/// 仍需由 A14 的 Serializable 事务重新锁定并验证全部事实。
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
    let mut requires_blacklist = false;
    for action in &entry.actions {
        if action.state != "satisfied" {
            continue;
        }
        if action.kind == "buyer_blacklist" {
            blacklist_satisfied = true;
            continue;
        }
        let next = match (entry.world_id.as_str(), action.kind.as_str()) {
            ("main", "reclaim_equipment_take") => Some("reclaimed"),
            ("isekai", "cancel_carrier_take") => Some("cancelled"),
            ("isekai", "recover_equipment_take") => Some("equipment_recovered"),
            _ => None,
        };
        let Some(next) = next else {
            continue;
        };
        if candidate.is_some_and(|current| current != next) {
            return Vec::new();
        }
        candidate = Some(next);
        requires_blacklist =
            matches!(action.kind.as_str(), "reclaim_equipment_take" | "cancel_carrier_take");
    }

    match (candidate, requires_blacklist, blacklist_satisfied) {
        (Some(action), false, _) | (Some(action), true, true) => vec![action.to_owned()],
        _ => Vec::new(),
    }
}

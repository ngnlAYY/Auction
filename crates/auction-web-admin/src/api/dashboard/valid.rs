//! Dashboard 快照解码后的语义校验。
//! 键闭集与“必填可空”由分区结构的 `deny_unknown_fields` 与
//! `auction_contracts::deserialize_required_option` 承担；这里只检查类型表达不了的
//! 跨字段一致性与取值闭集，任一不符整份快照失败关闭。显式为 null 的分区外壳字段
//! （error_code、observed_at、未知分区的 data）与带 default 的轮询 diagnostics 按“没有”处理；
//! 后端从不输出这些 null。

use super::{
    AdminAlertingSection, AdminDashboardSnapshot, AdminExternalAccountsData, AdminFulfillmentData,
    AdminMailData, AdminRealtimeData, AdminReconciliationData,
};

pub(super) fn valid_dashboard_semantics(snapshot: &AdminDashboardSnapshot) -> bool {
    snapshot.mail.data.as_ref().is_none_or(valid_mail)
        && snapshot.realtime.data.as_ref().is_none_or(valid_realtime)
        && snapshot.external_accounts.data.as_ref().is_none_or(valid_external_accounts)
        && snapshot.fulfillment.data.as_ref().is_none_or(valid_fulfillment)
        && snapshot.reconciliation.data.as_ref().is_none_or(valid_reconciliation)
        && valid_alerting(&snapshot.alerting)
}

/// 轮询摘要复用邮件页的 DTO，但 dashboard 从不选择诊断；出现诊断说明 wire 形状漂移。
fn valid_mail(data: &AdminMailData) -> bool {
    data.pollers.iter().all(|poller| poller.diagnostics.is_none())
}

fn valid_realtime(data: &AdminRealtimeData) -> bool {
    data.total_lag >= 0
}

/// 等待领取的读取失败数只是其子集；没有等待领取时不能带等待年龄。
fn valid_fulfillment(data: &AdminFulfillmentData) -> bool {
    data.awaiting_collection >= 0
        && (0..=data.awaiting_collection).contains(&data.awaiting_collection_unreadable)
        && data.oldest_awaiting_collection_age_seconds >= 0
        && (data.awaiting_collection > 0 || data.oldest_awaiting_collection_age_seconds == 0)
}

/// 对账报告按业务世界汇总，最多 main 与 isekai 两项；在途件数、笔数与金额不得为负。
fn valid_reconciliation(data: &AdminReconciliationData) -> bool {
    data.worlds.len() <= 2
        && data.worlds.iter().all(|world| {
            world.custody_in_transit_count >= 0
                && world.solvency_in_transit_count >= 0
                && world.solvency_in_transit_credits >= 0
        })
}

fn valid_external_accounts(data: &AdminExternalAccountsData) -> bool {
    // 忙碌时长只在忙碌期间存在，且不得为负。
    let busy_age_consistent = match (data.mail.busy, data.mail.busy_age_seconds) {
        (true, Some(age)) => age >= 0,
        (false, None) => true,
        _ => false,
    };
    busy_age_consistent
        && matches!(
            data.maintenance.state.as_str(),
            "idle" | "running" | "succeeded" | "failed" | "canceled"
        )
        && valid_projection_status(&data.mail.pollers.status)
        && valid_projection_status(&data.credentials.status)
        && data.forum.write_unknown_count >= 0
}

/// 子投影只允许 `available|unavailable`；不可用时空列表不是“没有条目”的事实。
fn valid_projection_status(status: &str) -> bool {
    matches!(status, "available" | "unavailable")
}

/// 告警关闭时分区恒为 ok 且不带观测时间；开启时分区观测时间必须等于最近一次扫描时间，
/// 两者同时缺失表示尚未扫描过。unknown 分区没有数据，外壳已由 `decode_section` 校验。
fn valid_alerting(section: &AdminAlertingSection) -> bool {
    let Some(data) = &section.data else {
        return true;
    };
    if !data.enabled {
        return section.status == "ok" && section.observed_at.is_none();
    }
    section.observed_at == data.last_observed_at
}

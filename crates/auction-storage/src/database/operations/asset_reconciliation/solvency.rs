//! 偿付能力报告的只读快照与判定：按 world 读取应付、在途、在途待核对与余额观察，计算缺口
//! 结论。在途与应付的划分以持久寄出事实与余额观察时点为准（见 `queries::SOLVENCY_WORLD`），
//! 未知余额永远不判定为足够。

use auction_application::reconciliation::{
    SolvencyInTransitReason, SolvencyReport, SolvencyWorldReport,
};
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{WORLDS, open_snapshot, queries, query_failed, snapshot_unavailable};
use crate::database::DatabaseStore;

pub(super) async fn solvency_snapshot(
    store: &DatabaseStore,
    report: &mut SolvencyReport,
    balance_evidence_max_age_seconds: u64,
) -> Result<(), &'static str> {
    let max_age_seconds = i64::try_from(balance_evidence_max_age_seconds).unwrap_or(i64::MAX);
    let mut transaction = open_snapshot(store).await?;
    for world in WORLDS {
        let row = sqlx::query(queries::SOLVENCY_WORLD)
            .bind(world)
            .fetch_one(&mut *transaction)
            .await
            .map_err(query_failed)?;
        let pending_seller_payout_count: i64 =
            row.try_get("pending_seller_payout_count").map_err(query_failed)?;
        let pending_seller_payout_credits: i64 =
            row.try_get("pending_seller_payout_credits").map_err(query_failed)?;
        let pending_refund_count: i64 =
            row.try_get("pending_refund_count").map_err(query_failed)?;
        let pending_refund_credits: i64 =
            row.try_get("pending_refund_credits").map_err(query_failed)?;
        let in_transit_seller_payout_count: i64 =
            row.try_get("in_transit_seller_payout_count").map_err(query_failed)?;
        let in_transit_seller_payout_credits: i64 =
            row.try_get("in_transit_seller_payout_credits").map_err(query_failed)?;
        let in_transit_refund_count: i64 =
            row.try_get("in_transit_refund_count").map_err(query_failed)?;
        let in_transit_refund_credits: i64 =
            row.try_get("in_transit_refund_credits").map_err(query_failed)?;
        let pending_outbound_mail_count: i64 =
            row.try_get("pending_outbound_mail_count").map_err(query_failed)?;
        let (in_transit_review_count, in_transit_review_credits, in_transit_review_reasons) =
            review_reasons(&row)?;
        let postage_estimate_credits = pending_outbound_mail_count
            .checked_mul(report.postage_rate_credits)
            .ok_or("reconciliation_query_failed")?;
        let required_credits = pending_seller_payout_credits
            .checked_add(pending_refund_credits)
            .and_then(|total| total.checked_add(postage_estimate_credits))
            .ok_or("reconciliation_query_failed")?;
        let balance_credits: Option<i64> = row.try_get("balance_credits").map_err(query_failed)?;
        let balance_observed_at: Option<DateTime<Utc>> =
            row.try_get("balance_observed_at").map_err(query_failed)?;
        let balance_status: Option<String> = row.try_get("balance_status").map_err(query_failed)?;
        let evidence_status: Option<String> =
            row.try_get("evidence_status").map_err(query_failed)?;
        let balance_age_seconds: Option<i64> =
            row.try_get("balance_age_seconds").map_err(query_failed)?;
        let mut balance_status = balance_status.unwrap_or_else(|| "unknown".to_owned());
        let mut evidence_status = evidence_status.unwrap_or_else(|| "unknown".to_owned());
        // 持久化的 fresh 只是上次刷新时的结论；观察年龄超过边界时必须按过期处理，
        // 否则刷新停止后旧的 ok/fresh 会继续产出 solvent=true。
        if evidence_status == "fresh"
            && !balance_evidence_is_current(balance_age_seconds, max_age_seconds)
        {
            balance_status = "stale".to_owned();
            evidence_status = "stale".to_owned();
        }
        let (verdict, shortfall_credits) = solvency_verdict(
            required_credits,
            balance_credits,
            &balance_status,
            &evidence_status,
            balance_age_seconds,
            max_age_seconds,
        );
        report.worlds.push(SolvencyWorldReport {
            world: world.to_owned(),
            pending_seller_payout_count,
            pending_seller_payout_credits,
            pending_refund_count,
            pending_refund_credits,
            in_transit_seller_payout_count,
            in_transit_seller_payout_credits,
            in_transit_refund_count,
            in_transit_refund_credits,
            in_transit_review_count,
            in_transit_review_credits,
            in_transit_review_reasons,
            pending_outbound_mail_count,
            postage_estimate_credits,
            required_credits,
            balance_credits,
            balance_observed_at,
            balance_status,
            evidence_status,
            verdict: verdict.to_owned(),
            shortfall_credits,
        });
    }
    transaction.commit().await.map_err(snapshot_unavailable)
}

/// 在途待核对的按原因明细：同一行里按原因排序的并列数组，逐项拼回并累计笔数与金额；数组长度
/// 不一致或累计溢出时失败关闭。
fn review_reasons(
    row: &sqlx::postgres::PgRow,
) -> Result<(i64, i64, Vec<SolvencyInTransitReason>), &'static str> {
    let reasons: Vec<String> = row.try_get("in_transit_review_reasons").map_err(query_failed)?;
    let counts: Vec<i64> = row.try_get("in_transit_review_counts").map_err(query_failed)?;
    let credits: Vec<i64> = row.try_get("in_transit_review_credits").map_err(query_failed)?;
    if counts.len() != reasons.len() || credits.len() != reasons.len() {
        return Err("reconciliation_query_failed");
    }
    let mut total_count = 0_i64;
    let mut total_credits = 0_i64;
    let mut details = Vec::with_capacity(reasons.len());
    for ((reason, count), credits) in reasons.into_iter().zip(counts).zip(credits) {
        total_count = total_count.checked_add(count).ok_or("reconciliation_query_failed")?;
        total_credits = total_credits.checked_add(credits).ok_or("reconciliation_query_failed")?;
        details.push(SolvencyInTransitReason { reason, count, credits });
    }
    Ok((total_count, total_credits, details))
}

/// 余额已被成功观察且未过期时才允许判定；其余情况一律 `unknown`，不以缺失值当作足够。
/// `balance_age_seconds` 来自数据库时钟；缺失、负数（时钟倒退）或超过调用方传入的
/// 有效配置边界都视为过期，即使持久化标签仍写着 `fresh`。
pub(super) fn solvency_verdict(
    required_credits: i64,
    balance_credits: Option<i64>,
    balance_status: &str,
    evidence_status: &str,
    balance_age_seconds: Option<i64>,
    max_age_seconds: i64,
) -> (&'static str, i64) {
    let fresh = matches!(balance_status, "ok" | "low")
        && evidence_status == "fresh"
        && balance_age_seconds.is_some_and(|age| (0..=max_age_seconds).contains(&age));
    match balance_credits.filter(|credits| *credits >= 0).filter(|_| fresh) {
        Some(balance) => {
            let shortfall = required_credits.saturating_sub(balance);
            if shortfall > 0 { ("shortfall", shortfall) } else { ("sufficient", 0) }
        }
        None => ("unknown", 0),
    }
}

/// 余额证据年龄是否仍在调用方配置的可信边界内。
pub(super) fn balance_evidence_is_current(
    balance_age_seconds: Option<i64>,
    max_age_seconds: i64,
) -> bool {
    balance_age_seconds.is_some_and(|age| (0..=max_age_seconds).contains(&age))
}

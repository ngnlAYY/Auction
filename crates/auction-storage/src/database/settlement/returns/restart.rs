//! 结算退件 Take 不经领取证明的两条机器复核出口，都由案件 owner 在锁内按持久事实判定：
//!
//! - 确定未外写：Take 投递终态 failed，且结果明确记录未开始写出、不是未知、从未因未知结果被巡检
//!   重排、没有远端引用。邮件没有被触碰，同一 Take 投递以新 fence 重新排队、案件留在 Take 待执行；
//!   每案最多重启 [`MAX_NOT_STARTED_RESTARTS`] 次，用满后转结果未知。其余失败与未知仍只认领取证明。
//! - 激活前冲突的交接：另一 owner 对同一邮件的意图已以持久事实确定从未执行（终态 failed、从未进入
//!   发送围栏、没有发送或领取尝试）时，案件从结果未知回到 Take 待执行，由下一次激活在 world 锁下
//!   按同一口径复核互斥；仍有任何可能已执行或尚在进行的意图时保持不动，不以普通重试绕过互斥。
//!
//! 两者都只追加案件事件、不写领回事实或资金（不变量 4、6）。

use auction_application::settlement::SettlementReturnProgress;
use auction_domain::SettlementReturnCaseStatus;
use auction_time::Timestamp;
use serde_json::json;
use sqlx::Postgres;

use super::cases::{CaseEvent, CaseRow, CaseTransition, transition};
use super::errors::ReturnTransactionError;
use super::take::{ActionRow, take_command_id};
use crate::database::TrackedTransaction as Transaction;
use crate::database::mail::delivery::disposition::settlement_return_take_conflicts;
use crate::database::mail::delivery::restart::restart_unstarted_take_in_transaction;

/// 每个案件允许的“确定未外写”重启次数；用满后转结果未知，防止确定失败的 Take 无限循环。
pub(super) const MAX_NOT_STARTED_RESTARTS: i64 = 2;

/// 事件里标记“确定未外写重启”的原因码。
const NOT_STARTED_REASON: &str = "take_not_started";

/// 确定未外写的 Take：未用满重启次数时把同一投递以新 fence 重新排队，案件留在 Take 待执行并追加
/// 一条 `take_activated` 事件；返回 `None` 表示已用满次数或投递已不满足条件，由调用方转结果未知。
/// 调用方已按锁序持有案件、动作与投递行锁。
pub(super) async fn restart_unstarted_take(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    action: &ActionRow,
    now: Timestamp,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    let Some(delivery_id) = action.delivery_id else {
        return Ok(None);
    };
    let restarts: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM public.settlement_return_events
           WHERE return_mail_id = $1 AND event_kind = 'take_activated'
             AND details ->> 'reason' = $2"#,
    )
    .bind(case.return_mail_id)
    .bind(NOT_STARTED_REASON)
    .fetch_one(&mut **tx)
    .await?;
    if restarts >= MAX_NOT_STARTED_RESTARTS
        || !restart_unstarted_take_in_transaction(tx, delivery_id, restarts + 1).await?
    {
        return Ok(None);
    }
    let next = CaseTransition::from_case(case, SettlementReturnCaseStatus::TakePending);
    let version = transition(
        tx,
        case,
        &next,
        CaseEvent {
            kind: "take_activated",
            command: None,
            details: json!({
                "delivery_id": delivery_id,
                "requeued": true,
                "reason": NOT_STARTED_REASON,
                "restarts": restarts + 1,
            }),
        },
        now,
    )
    .await?;
    Ok(Some(SettlementReturnProgress {
        return_mail_id: case.return_mail_id,
        status: SettlementReturnCaseStatus::TakePending,
        case_version: version,
        event_kind: "take_activated",
    }))
}

/// 激活前冲突的案件（结果未知、动作仍是没有投递的 pending）：world 锁下按交接口径复核同邮件
/// 互斥，已没有阻断意图时回到 Take 待执行并追加 `take_requested` 事件；仍被阻断返回 `None`。
/// 调用方已持有 world、案件与动作行锁。
pub(super) async fn hand_over_conflicted_take(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    action: &ActionRow,
    now: Timestamp,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    if action.delivery_id.is_some()
        || action.execution_state != "pending"
        || settlement_return_take_conflicts(
            tx,
            &action.world_id,
            &action.source_external_mail_id,
            take_command_id(action.return_mail_id).as_uuid(),
        )
        .await?
    {
        return Ok(None);
    }
    let next = CaseTransition::from_case(case, SettlementReturnCaseStatus::TakePending);
    let version = transition(
        tx,
        case,
        &next,
        CaseEvent { kind: "take_requested", command: None, details: json!({"handover": true}) },
        now,
    )
    .await?;
    Ok(Some(SettlementReturnProgress {
        return_mail_id: case.return_mail_id,
        status: SettlementReturnCaseStatus::TakePending,
        case_version: version,
        event_kind: "take_requested",
    }))
}

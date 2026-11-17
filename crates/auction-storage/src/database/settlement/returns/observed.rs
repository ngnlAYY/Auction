//! Take 结果未知（`take_unknown`）的原因：邮箱维护在原 Take 会话里观察到、不推进案件的事实以
//! 事件 `take_observed` 只追加留证；没有观察时由持久事实推出原因。详情、告警与再次领回的准入
//! 都读这一处口径。
//!
//! - 留证只追加案件事件（推进案件版本，状态不变），不改写投递、动作、资金或证据；与案件最近
//!   一次事件是同一原因、同一 Take fence 时不重复写，巡检每轮复核不会无界增长（不变量 8）。
//! - 只有案件最近一次事件正是当前 Take fence 上的 `assets_claimable` 观察时，才证明上一次 Take
//!   没有取走资产、原完整资产仍可领取；这是 Admin 再次领回的唯一前提，不接受操作者声明
//!   （不变量 4、6）。
//!
//! 锁序：调用方已按“观测 generation → 凭据”取锁；本模块依次锁案件 → 动作 → 投递，是全局锁序
//! “generation → world → 资金 → 恢复与义务 → 凭据 → 案件 → 动作 → 投递”的子序列（不变量 2）。

use auction_application::delivery::DeliveryTakeExpectation;
use auction_application::mail::MailDeliveryOrigin;
use auction_application::settlement::SettlementReturnProgress;
use auction_domain::{SettlementReturnCaseStatus, SettlementReturnTakeUnknownReason};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::Postgres;
use uuid::Uuid;

use super::cases::{CaseEvent, CaseTransition, load_case, transition};
use super::errors::ReturnTransactionError;
use super::take::{load_action, take_origin_id};
use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::scheduler::take_attempts;

/// 原会话观察事件。
pub(super) const TAKE_OBSERVED_EVENT: &str = "take_observed";

/// Take 结果未知的原因（`$case` 为 `settlement_return_cases` 的别名；非 `take_unknown` 时为 NULL）：
///
/// 1. 案件最近一次事件是 `take_observed` 时取它记录的观察原因（邮箱维护在原会话里的事实）；
/// 2. 否则由持久事实推出：动作没有 Take 投递（激活前同邮件冲突）为 `disposition_conflict`；
///    Take 投递不在人工核对（以失败或回执不符终结，巡检不复核）为 `take_unverifiable`；Take 冻结
///    的凭据已不是同版本的 active 凭据（或没有冻结凭据）为 `session_changed`；其余为
///    `awaiting_observation`。
macro_rules! take_unknown_reason_sql {
    ($case:literal) => {
        concat!(
            "CASE WHEN ",
            $case,
            ".status <> 'take_unknown' THEN NULL ELSE COALESCE(
                (SELECT CASE WHEN latest.event_kind = 'take_observed'
                             THEN latest.details ->> 'observation' END
                 FROM public.settlement_return_events AS latest
                 WHERE latest.return_mail_id = ",
            $case,
            ".return_mail_id
                 ORDER BY latest.case_version DESC
                 LIMIT 1),
                (SELECT CASE
                     WHEN taking.delivery_id IS NULL THEN 'disposition_conflict'
                     WHEN taken.status IS DISTINCT FROM 'manual_reconcile' THEN 'take_unverifiable'
                     WHEN NOT EXISTS (
                         SELECT 1
                         FROM public.delivery_current_take_attempt(taken.id) AS attempt
                         JOIN public.credential_keys AS credential
                           ON credential.id::text
                              = attempt.expectation -> 'credential' ->> 'credential_id'
                         WHERE credential.credential_kind = 'mail_cookie'
                           AND credential.status = 'active'
                           AND credential.state_version::text
                               = attempt.expectation -> 'credential' ->> 'credential_version'
                     ) THEN 'session_changed'
                     ELSE 'awaiting_observation'
                 END
                 FROM public.settlement_return_actions AS taking
                 LEFT JOIN public.deliveries AS taken ON taken.id = taking.delivery_id
                 WHERE taking.return_mail_id = ",
            $case,
            ".return_mail_id),
                'awaiting_observation') END"
        )
    };
}

pub(crate) use take_unknown_reason_sql;

/// 读取一个案件的 Take 结果未知原因；非 `take_unknown` 时为 `None`，原因越出闭集时失败关闭。
pub(super) async fn take_unknown_reason(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
) -> Result<Option<SettlementReturnTakeUnknownReason>, ReturnTransactionError> {
    let reason: Option<String> = sqlx::query_scalar(concat!(
        "SELECT ",
        take_unknown_reason_sql!("c"),
        " FROM public.settlement_return_cases AS c WHERE c.return_mail_id = $1"
    ))
    .bind(return_mail_id)
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    reason
        .map(|reason| {
            SettlementReturnTakeUnknownReason::from_name(&reason)
                .ok_or_else(|| crate::database::repository_error("结算退件结果未知原因无效"))
        })
        .transpose()
        .map_err(Into::into)
}

/// 案件最近一次事件是否正是当前 Take fence 上的 `assets_claimable` 观察，且 Take 投递与动作仍
/// 停在人工核对、没有远端引用：上一次 Take 没有取走资产，原完整资产仍可领取。
pub(super) async fn take_retry_proven(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
) -> Result<bool, ReturnTransactionError> {
    let proven: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
               FROM (SELECT event_kind, details
                     FROM public.settlement_return_events
                     WHERE return_mail_id = $1
                     ORDER BY case_version DESC
                     LIMIT 1) AS latest
               JOIN public.settlement_return_actions AS action ON action.return_mail_id = $1
               JOIN public.deliveries AS taken ON taken.id = action.delivery_id
               WHERE latest.event_kind = 'take_observed'
                 AND latest.details ->> 'observation' = 'assets_claimable'
                 AND latest.details ->> 'delivery_id' = taken.id::text
                 AND latest.details -> 'fencing_version' = to_jsonb(taken.fencing_version)
                 AND action.execution_state = 'manual_reconcile'
                 AND taken.status = 'manual_reconcile'
                 AND taken.origin_kind = 'settlement_return_action'
                 AND taken.external_reference IS NULL
           )"#,
    )
    .bind(return_mail_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(proven)
}

/// 一次原会话观察。
pub(crate) struct TakeObservation<'a> {
    pub(crate) take_delivery_id: Uuid,
    pub(crate) fencing_version: i64,
    pub(crate) reason: SettlementReturnTakeUnknownReason,
    pub(crate) sweep_retries: i64,
    pub(crate) expectation: &'a DeliveryTakeExpectation,
}

/// 记录一次原会话观察：Take 投递必须是结算退件来源、停在人工核对、当前 fence 与冻结 Take 尝试
/// 都与观察一致；案件停在结果未知、动作停在人工核对且正是这条投递。与最近一次事件相同或前提
/// 不成立时返回 `None`，不写任何事实。调用方已锁观测 generation 与凭据。
pub(crate) async fn record_take_observation(
    tx: &mut Transaction<'_, Postgres>,
    observation: &TakeObservation<'_>,
    now: Timestamp,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    if !observation.reason.observed() || observation.fencing_version <= 0 {
        return Ok(None);
    }
    let origin: Option<(String, String)> =
        sqlx::query_as("SELECT origin_kind, origin_id FROM public.deliveries WHERE id = $1")
            .bind(observation.take_delivery_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((origin_kind, origin_id)) = origin else {
        return Ok(None);
    };
    // 来源字段不可变；只写规范案件 UUID，无法逐字还原即无法证明归属。
    let Some(return_mail_id) = (origin_kind == MailDeliveryOrigin::SettlementReturnAction.as_str())
        .then(|| origin_id.strip_prefix("settlement_return:"))
        .flatten()
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| take_origin_id(*value) == origin_id)
    else {
        return Ok(None);
    };
    let (Some(case), Some(action)) =
        (load_case(tx, return_mail_id, true).await?, load_action(tx, return_mail_id, true).await?)
    else {
        return Ok(None);
    };
    if case.status != SettlementReturnCaseStatus::TakeUnknown
        || action.execution_state != "manual_reconcile"
        || action.delivery_id != Some(observation.take_delivery_id)
    {
        return Ok(None);
    }
    let delivery: Option<(String, Option<String>)> = sqlx::query_as(
        r#"SELECT status, external_reference FROM public.deliveries
           WHERE id = $1 AND fencing_version = $2 FOR UPDATE"#,
    )
    .bind(observation.take_delivery_id)
    .bind(observation.fencing_version)
    .fetch_optional(&mut **tx)
    .await?;
    if !matches!(delivery, Some((ref status, None)) if status == "manual_reconcile") {
        return Ok(None);
    }
    let Some((_, frozen)) =
        take_attempts::load_current(tx, observation.take_delivery_id, observation.fencing_version)
            .await?
    else {
        return Ok(None);
    };
    if &frozen != observation.expectation {
        return Ok(None);
    }
    if latest_observation_matches(tx, return_mail_id, observation).await? {
        return Ok(None);
    }
    let next = CaseTransition::from_case(&case, SettlementReturnCaseStatus::TakeUnknown);
    let version = transition(
        tx,
        &case,
        &next,
        CaseEvent {
            kind: TAKE_OBSERVED_EVENT,
            command: None,
            details: observation_details(observation),
        },
        now,
    )
    .await?;
    Ok(Some(SettlementReturnProgress {
        return_mail_id,
        status: SettlementReturnCaseStatus::TakeUnknown,
        case_version: version,
        event_kind: TAKE_OBSERVED_EVENT,
    }))
}

fn observation_details(observation: &TakeObservation<'_>) -> Value {
    json!({
        "observation": observation.reason.as_str(),
        "delivery_id": observation.take_delivery_id,
        "fencing_version": observation.fencing_version,
        "sweep_retries": observation.sweep_retries,
    })
}

/// 案件最近一次事件已是同一原因、同一投递与 fence 的观察：重复观察不再追加。
async fn latest_observation_matches(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    observation: &TakeObservation<'_>,
) -> Result<bool, ReturnTransactionError> {
    let latest: Option<(String, Value)> = sqlx::query_as(
        r#"SELECT event_kind, details FROM public.settlement_return_events
           WHERE return_mail_id = $1
           ORDER BY case_version DESC
           LIMIT 1"#,
    )
    .bind(return_mail_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(latest.is_some_and(|(kind, details)| {
        kind == TAKE_OBSERVED_EVENT
            && details.get("observation").and_then(Value::as_str)
                == Some(observation.reason.as_str())
            && details.get("delivery_id").and_then(Value::as_str)
                == Some(observation.take_delivery_id.to_string().as_str())
            && details.get("fencing_version").and_then(Value::as_i64)
                == Some(observation.fencing_version)
    }))
}

//! 恢复记录当前退件与原义务发送尝试的归属：Admin 确认收口与 Worker 自动重寄、退回卖家共用的
//! 唯一判定，Worker 自动推进时归属不成立的阻断写入与机器解除，以及恢复报告里的具体原因。
//!
//! 一封邮件只会被退回一次，但退件之前同一义务可能不止一次发送到达远端（结果未知后的重试、
//! 旧版本 Admin 缺口的重新激活）。时间截止只能排除退件之后的发送，不能说明退件属于哪一次：同一
//! 分钟内的多次发送、或时钟偏差让后一次发送落在截止之前，都会通过截止判断。这里只认持久事实：
//! 本义务全部投递上每一次可能已到达远端的发送，都必须对应一封本恢复记录已消费的系统退件（两者
//! 计数相等），并且没有任何发送晚于当前退件的截止；或者每一次可能外达的发送都有 Sent 观察到的
//! 远端 MID，MID 与已消费退件一一对应（补充证明，见 `predicate`）。可能外达的发送多于已消费的
//! 退件时，至少有一封邮件可能仍在对方手里，收口或重寄会与它冲突，调用方失败关闭。判定本身只读；
//! 阻断与解除只写恢复记录的阻断码与审计，不改投递、履约、资金或成交。

mod predicate;

use auction_application::recovery::RecoveryCommandError;
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgConnection, Postgres};
use uuid::Uuid;

use self::predicate::SEND_ATTRIBUTION_SQL;
use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::commands::confirm::insert_worker_recovery_audit;
use crate::database::recovery::commands::mapping::map_sql_error;

/// Worker 自动推进时归属不成立的阻断码；恢复报告对外归并为 `recovery_evidence_mismatch`，具体
/// 原因另由 [`SendAttributionBlocker`] 给出。
pub(super) const SEND_UNATTRIBUTED_CODE: &str = "send_unattributed";

/// 阻断后到下一次机器复核的间隔：新的 Sent 绑定、或原投递被终结之后归属可能成立；间隔只控制
/// 复核频率，复核仍在锁内按持久事实判定。
const SEND_ATTRIBUTION_RECHECK_MINUTES: i64 = 10;

/// 归属成立所依据的规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SendAttributionRule {
    /// 计数相等且没有发送晚于当前退件截止。
    Count,
    /// 每次可能外达的发送都有 Sent 观察到的远端 MID，与已消费退件一一对应。
    SentMid,
}

impl SendAttributionRule {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::SentMid => "sent_mid",
        }
    }
}

/// 归属不成立的具体原因（闭集，恢复报告按原值输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendAttributionBlocker {
    /// 当前退件不在已消费退件中或出现多次：持久事实不一致。
    ReturnNotCurrent,
    /// 义务的结算投递仍在准备或发送中，或当前派发之外仍有排队的投递。
    SendInFlight,
    /// 每次发送都有 Sent MID，但多于已消费的退件：至少一封邮件仍在对方手里。
    SendsExceedReturns,
    /// 发送多于已消费的退件，且有发送没有 Sent 观察到的 MID，无法证明它未到达远端。
    SendWithoutSentMid,
    /// 发送少于退件，或第一轮退件之前看不到发送：发送记录不完整。
    SendRecordsIncomplete,
    /// 有发送不早于当前退件的截止，且没有完整的 Sent MID 可以代替时间比较。
    SendAfterReturn,
}

impl SendAttributionBlocker {
    /// 恢复报告使用的稳定名称。
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ReturnNotCurrent => "return_not_current",
            Self::SendInFlight => "send_in_flight",
            Self::SendsExceedReturns => "sends_exceed_returns",
            Self::SendWithoutSentMid => "send_without_sent_mid",
            Self::SendRecordsIncomplete => "send_records_incomplete",
            Self::SendAfterReturn => "send_after_return",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [
            Self::ReturnNotCurrent,
            Self::SendInFlight,
            Self::SendsExceedReturns,
            Self::SendWithoutSentMid,
            Self::SendRecordsIncomplete,
            Self::SendAfterReturn,
        ]
        .into_iter()
        .find(|blocker| blocker.as_str() == value)
    }
}

/// 一次归属判定的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SendAttribution {
    Attributed(SendAttributionRule),
    Blocked(SendAttributionBlocker),
}

/// 判定恢复记录已消费的退件能否把义务的全部可能外达发送逐一归属（口径见 `predicate`）。只读；
/// 结果行形状不符时返回 `None`，由调用方按持久事实无效失败关闭。
async fn evaluate(
    connection: &mut PgConnection,
    obligation_id: Uuid,
    recovery_id: Uuid,
) -> Result<Option<SendAttribution>, sqlx::Error> {
    let (blocker, rule): (Option<String>, Option<String>) = sqlx::query_as(SEND_ATTRIBUTION_SQL)
        .bind(obligation_id)
        .bind(recovery_id)
        .fetch_one(connection)
        .await?;
    Ok(match (blocker.as_deref(), rule.as_deref()) {
        (None, Some("count")) => Some(SendAttribution::Attributed(SendAttributionRule::Count)),
        (None, Some("sent_mid")) => Some(SendAttribution::Attributed(SendAttributionRule::SentMid)),
        (Some(blocker), None) => {
            SendAttributionBlocker::parse(blocker).map(SendAttribution::Blocked)
        }
        _ => None,
    })
}

/// 命令事务内的归属判定。调用方已按既有锁序锁定恢复记录与义务的当前投递；不成立时不得收口
/// 付款或交付、追加处置或重列装备（不变量 2、4、6）。
pub(super) async fn send_attribution(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    recovery_id: Uuid,
) -> Result<SendAttribution, RecoveryCommandError> {
    evaluate(transaction, obligation_id, recovery_id)
        .await
        .map_err(map_sql_error)?
        .ok_or(RecoveryCommandError::InvalidStoredResult)
}

/// 归属成立时返回真；见 [`send_attribution`]。
pub(super) async fn return_attributable_to_sends(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    recovery_id: Uuid,
) -> Result<bool, RecoveryCommandError> {
    Ok(matches!(
        send_attribution(transaction, obligation_id, recovery_id).await?,
        SendAttribution::Attributed(_)
    ))
}

/// 恢复报告读取的具体原因：只读快照内按同一判定求值，归属成立时为 `None`。结果形状不符时返回
/// 协议错误，由报告读取方按读取失败处理。
pub(crate) async fn send_attribution_blocker(
    connection: &mut PgConnection,
    obligation_id: Uuid,
    recovery_id: Uuid,
) -> Result<Option<SendAttributionBlocker>, sqlx::Error> {
    match evaluate(connection, obligation_id, recovery_id).await? {
        Some(SendAttribution::Attributed(_)) => Ok(None),
        Some(SendAttribution::Blocked(blocker)) => Ok(Some(blocker)),
        None => Err(sqlx::Error::Protocol("发送归属判定结果形状无效".to_owned())),
    }
}

/// Worker 自动推进（重寄或退回卖家）遇到归属不成立：写入阻断码并退出自动推进，阶段保持
/// `reclaiming`，同事务追加审计 `recovery.send_attribution_blocked`（带具体原因）。不撤销成交、
/// 不重寄、不追加处置；调用方提交后返回，不能报错，否则按更新时间排序的队头每轮重选同一行
/// （不变量 4、6、8）。下一次机器复核排在固定间隔之后。返回新的状态版本。
pub(super) async fn block_unattributed_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
    state_version: i64,
    blocker: SendAttributionBlocker,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let (next_state_version, return_mail_id): (i64, Option<Uuid>) = sqlx::query_as(
        r#"UPDATE public.settlement_recoveries
           SET blocking_code = $4, next_attempt_at = $5,
               state_version = state_version + 1, updated_at = $2
           WHERE id = $1 AND stage = 'reclaiming' AND state_version = $3
             AND blocking_code IS NULL
           RETURNING state_version, return_mail_id"#,
    )
    .bind(recovery_id)
    .bind(now)
    .bind(state_version)
    .bind(SEND_UNATTRIBUTED_CODE)
    .bind(recheck_at(now)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    insert_worker_recovery_audit(
        transaction,
        recovery_id,
        "recovery.send_attribution_blocked",
        now,
        serde_json::json!({
            "blocking_code": SEND_UNATTRIBUTED_CODE,
            "reason": blocker.as_str(),
            "return_mail_id": return_mail_id,
            "state_version": next_state_version,
        }),
    )
    .await?;
    Ok(next_state_version)
}

/// 已阻断的恢复记录经机器复核后归属成立：清除阻断码，同事务追加审计
/// `recovery.send_attribution_released`（记录依据的规则），调用方随即在同一事务继续自动推进。
/// 只解除 `send_unattributed`，不接受操作者声明（不变量 6、8）。返回新的状态版本。
pub(super) async fn release_attributed_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
    state_version: i64,
    rule: SendAttributionRule,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let (next_state_version, return_mail_id): (i64, Option<Uuid>) = sqlx::query_as(
        r#"UPDATE public.settlement_recoveries
           SET blocking_code = NULL, next_attempt_at = NULL,
               state_version = state_version + 1, updated_at = $2
           WHERE id = $1 AND stage = 'reclaiming' AND state_version = $3
             AND blocking_code = $4
           RETURNING state_version, return_mail_id"#,
    )
    .bind(recovery_id)
    .bind(now)
    .bind(state_version)
    .bind(SEND_UNATTRIBUTED_CODE)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    insert_worker_recovery_audit(
        transaction,
        recovery_id,
        "recovery.send_attribution_released",
        now,
        serde_json::json!({
            "released_code": SEND_UNATTRIBUTED_CODE,
            "rule": rule.as_str(),
            "return_mail_id": return_mail_id,
            "state_version": next_state_version,
        }),
    )
    .await?;
    Ok(next_state_version)
}

/// 机器复核后归属仍不成立：只把下一次复核推迟一个间隔，不改状态版本（Admin 页面持有的版本
/// 不因复核失效），不写审计。
pub(super) async fn defer_unattributed_recheck(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
    state_version: i64,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET next_attempt_at = $4
           WHERE id = $1 AND stage = 'reclaiming' AND state_version = $2
             AND blocking_code = $3"#,
    )
    .bind(recovery_id)
    .bind(state_version)
    .bind(SEND_UNATTRIBUTED_CODE)
    .bind(recheck_at(now)?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(RecoveryCommandError::StaleState);
    }
    Ok(())
}

fn recheck_at(now: DateTime<Utc>) -> Result<DateTime<Utc>, RecoveryCommandError> {
    now.checked_add_signed(Duration::minutes(SEND_ATTRIBUTION_RECHECK_MINUTES))
        .ok_or(RecoveryCommandError::InvalidStoredResult)
}

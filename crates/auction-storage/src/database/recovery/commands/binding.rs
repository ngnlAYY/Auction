//! 恢复记录当前退件与原义务发送尝试的归属：Admin 确认收口时的唯一判定。
//!
//! 一封邮件只会被退回一次，但退件之前同一义务可能不止一次发送到达远端（结果未知后的重试、
//! 旧版本 Admin 缺口的重新激活）。时间截止只能排除退件之后的发送，不能说明退件属于哪一次：同一
//! 分钟内的多次发送、或时钟偏差让后一次发送落在截止之前，都会通过截止判断。这里只认持久事实：
//! 本义务全部投递上每一次可能已到达远端的发送，都必须对应一封本恢复记录已消费的系统退件（两者
//! 计数相等），并且没有任何发送晚于当前退件的截止。可能外达的发送多于已消费的退件时，至少有
//! 一封邮件可能仍在对方手里，收口会与它冲突，调用方失败关闭、交人工核对。判定只读，不写任何状态。

use auction_application::recovery::RecoveryCommandError;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::settlement::fulfillment::system_settlement_return_source;

/// 恢复记录 `$2` 已消费的退件能否把义务 `$1` 的全部发送逐一归属。
///
/// - 发送：义务全部结算投递上的冻结发送尝试；没有冻结尝试的旧发送围栏（投递当前 `sending_at`）；
///   恢复后外部核对在 Sent 里找到、库里没有冻结尝试的隔离发送；没有冻结尝试的旧 Sent 绑定（当前
///   旧发送围栏自身的绑定除外）。只有投递当前这次发送能凭投递行证明确定没有外写：终态 `failed`、
///   没有远端引用、结果明确记录 `write_started=false` 与 `outcome_unknown=false`（缺键、null 或
///   错误类型都不算），且当前发送 fence 上没有 Sent 绑定；历史尝试的结果已被后续派发覆盖，一律
///   按可能已到达远端计数。
/// - 退件：本恢复记录已消费、绑定本义务、来源可证明为系统 `[RTS]` 的退件；截止取消费时刻与收件
///   时刻加 3 分钟（收件箱时间只到分钟，另留 2 分钟远端时钟余量）中较早者。
///
/// 成立条件同时满足：当前退件在其中且恰好一封；没有任何发送不早于当前退件的截止；可能外达的
/// 发送数等于退件数；最早的发送早于最早退件的截止（旧版本没有冻结尝试、又被重寄清掉的发送不会
/// 留在库里，第一轮退件之前看不到任何发送即说明发送记录不完整）。任一不满足、或事实缺失时为假。
const RETURN_ATTRIBUTION: &str = concat!(
    r#"WITH obligation_deliveries AS (
    SELECT delivery.id,
           delivery.status,
           delivery.sending_at,
           delivery.fencing_version,
           delivery.external_reference,
           delivery.result_metadata,
           current_attempt.sending_fencing_version AS current_fence
    FROM public.deliveries AS delivery
    LEFT JOIN LATERAL public.delivery_current_send_attempt(delivery.id) AS current_attempt
      ON true
    WHERE delivery.origin_kind = 'settlement' AND delivery.origin_id = $1::text
),
proven_unsent AS (
    SELECT candidate.id
    FROM obligation_deliveries AS candidate
    WHERE candidate.status = 'failed'
      AND candidate.external_reference IS NULL
      AND COALESCE(candidate.result_metadata
              @> '{"write_started": false, "outcome_unknown": false}'::jsonb, false)
      AND NOT EXISTS (
          SELECT 1
          FROM public.delivery_sent_bindings AS bound
          WHERE bound.delivery_id = candidate.id
            AND bound.fencing_version
                = COALESCE(candidate.current_fence, candidate.fencing_version)
      )
),
sends AS (
    SELECT attempt.sending_at,
           COALESCE(attempt.fencing_version = candidate.current_fence, false)
               AND candidate.id IN (SELECT id FROM proven_unsent) AS unsent
    FROM obligation_deliveries AS candidate
    JOIN public.delivery_send_attempts AS attempt ON attempt.delivery_id = candidate.id
    UNION ALL
    SELECT candidate.sending_at, candidate.id IN (SELECT id FROM proven_unsent)
    FROM obligation_deliveries AS candidate
    WHERE candidate.sending_at IS NOT NULL AND candidate.current_fence IS NULL
    UNION ALL
    SELECT quarantine.sent_at, false
    FROM obligation_deliveries AS candidate
    JOIN public.delivery_external_quarantines AS quarantine
      ON quarantine.delivery_id = candidate.id
    UNION ALL
    SELECT NULL::timestamptz, false
    FROM obligation_deliveries AS candidate
    JOIN public.delivery_sent_bindings AS bound ON bound.delivery_id = candidate.id
    WHERE bound.expectation_fencing_version IS NULL
      AND NOT (candidate.sending_at IS NOT NULL AND candidate.current_fence IS NULL
               AND bound.fencing_version = candidate.fencing_version)
),
returns AS (
    SELECT LEAST(consumed.recorded_at, inbound.received_at + interval '3 minutes') AS cutoff,
           inbound.id = recovery.return_mail_id AS current_return
    FROM public.settlement_recoveries AS recovery
    JOIN public.settlement_recovery_return_consumptions AS consumed
      ON consumed.recovery_id = recovery.id
    JOIN public.inbound_mail AS inbound ON inbound.id = consumed.return_mail_id
    WHERE recovery.id = $2
      AND inbound.decision_metadata ->> 'obligation_id' = $1::text
      AND "#,
    system_settlement_return_source!(),
    r#"
)
SELECT COALESCE(
    (SELECT count(*) FROM returns WHERE returns.current_return) = 1
    AND NOT EXISTS (
        SELECT 1
        FROM sends
        CROSS JOIN returns
        WHERE returns.current_return AND sends.sending_at >= returns.cutoff
    )
    AND (SELECT count(*) FROM sends WHERE NOT sends.unsent) = (SELECT count(*) FROM returns)
    AND (SELECT min(sends.sending_at) FROM sends) < (SELECT min(returns.cutoff) FROM returns),
    false)"#
);

/// 恢复记录已消费的退件能把义务的全部可能外达发送逐一归属、且当前退件之后没有任何发送时返回
/// 真。调用方已按既有锁序锁定恢复记录与义务的当前投递；为假时不得收口付款或交付、追加处置或
/// 重列装备（不变量 2、4、6）。
pub(super) async fn return_attributable_to_sends(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    recovery_id: Uuid,
) -> Result<bool, RecoveryCommandError> {
    sqlx::query_scalar(RETURN_ATTRIBUTION)
        .bind(obligation_id)
        .bind(recovery_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)
}

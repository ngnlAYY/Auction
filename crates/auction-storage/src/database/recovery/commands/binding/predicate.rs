//! 发送归属判定的 SQL：一次查询同时求出计数规则、Sent MID 规则与不成立时的具体原因。
//!
//! 参数 `$1` 为被退回的义务，`$2` 为恢复记录。结果为一行：`blocker` 为空表示归属成立，此时
//! `rule` 为 `count`（计数与时间截止）或 `sent_mid`（Sent 观察到的远端 MID）；不成立时 `blocker`
//! 是 [`super::SendAttributionBlocker`] 闭集中的一个原因。

use crate::database::settlement::fulfillment::system_settlement_return_source;

/// 归属判定。
///
/// - 发送：义务全部结算投递上的冻结发送尝试（带该尝试 fence 上的 Sent 绑定 MID）；没有冻结尝试
///   的旧发送围栏（投递当前 `sending_at`）；恢复后外部核对隔离的发送；没有冻结尝试的旧 Sent 绑定
///   （当前旧发送围栏自身的绑定除外）。只有投递当前这次发送能凭投递行证明确定没有外写：终态
///   `failed`、没有远端引用、结果明确记录 `write_started=false` 与 `outcome_unknown=false`（缺键、
///   null 或错误类型都不算），且当前发送 fence 上没有 Sent 绑定；历史尝试的结果已被后续派发覆盖，
///   一律按可能已到达远端计数。
/// - 退件：本恢复记录已消费、绑定本义务、来源可证明为系统 `[RTS]` 的退件；截止取消费时刻与收件
///   时刻加 3 分钟（收件箱时间只到分钟，另留 2 分钟远端时钟余量）中较早者。
/// - 在途：义务的结算投递仍在准备或发送中，或除履约当前派发之外仍有排队的投递。当前派发停在
///   pending/claimed 且当前 fence 从未进入发送围栏（`sending_at` 为空）时不算：恢复接管退件后
///   投递门禁让它不会再被领取或发送，收口时由恢复 owner 同事务终结（重寄时由恢复自己的重寄接替）。
///
/// 计数规则（原规则）：当前退件恰好一封；没有任何发送不早于当前退件的截止；可能外达的发送数等于
/// 退件数；最早的发送早于最早退件的截止（旧版本没有冻结尝试、又被重寄清掉的发送不会留在库里，
/// 第一轮退件之前看不到任何发送即说明发送记录不完整）；没有在途投递。
///
/// Sent MID 规则（补充证明，不放宽计数规则）：每一次可能外达的发送都是冻结发送尝试，且该尝试
/// fence 上有 Sent 观察到的远端 MID（`delivery_sent_bindings`，全局唯一）；MID 互不相同、数量等于
/// 已消费的退件数，当前退件恰好一封，最早的发送早于最早退件的截止，没有在途投递。每封邮件只会
/// 被系统退回一次，已到达远端的邮件恰好就是这些 MID，退件数与之相等即每个 MID 都对应一封已消费
/// 退件（一一归属），没有仍在对方手里的邮件；本地时钟与远端分钟时间的截止比较因此不再需要。
/// 旧发送围栏、旧 Sent 绑定与外部核对隔离说明发送记录可能不完整，不适用本规则。
///
/// 不成立的原因按顺序取第一个：`return_not_current`（当前退件缺失或重复）、`send_in_flight`、
/// `sends_exceed_returns`（全部发送都有 MID 且多于退件：至少一封邮件仍在对方手里）、
/// `send_without_sent_mid`（发送多于退件，且有发送没有 MID，无法证明它未到达远端）、
/// `send_records_incomplete`（发送少于退件或第一轮看不到发送）、`send_after_return`（有发送不早于
/// 当前退件截止，且没有完整的 MID 可以代替时间比较）。
pub(super) const SEND_ATTRIBUTION_SQL: &str = concat!(
    r#"WITH obligation_deliveries AS (
    SELECT delivery.id,
           delivery.status,
           delivery.sending_at,
           delivery.fencing_version,
           delivery.external_reference,
           delivery.result_metadata,
           current_attempt.sending_fencing_version AS current_fence,
           COALESCE(delivery.id = (
               SELECT fulfillment.delivery_id
               FROM public.settlement_obligation_fulfillments AS fulfillment
               WHERE fulfillment.obligation_id = $1
           ), false) AS current_dispatch
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
               AND candidate.id IN (SELECT id FROM proven_unsent) AS unsent,
           true AS frozen,
           attempt_binding.world_id || ':' || attempt_binding.external_mail_id AS mid
    FROM obligation_deliveries AS candidate
    JOIN public.delivery_send_attempts AS attempt ON attempt.delivery_id = candidate.id
    LEFT JOIN public.delivery_sent_bindings AS attempt_binding
      ON attempt_binding.delivery_id = attempt.delivery_id
     AND attempt_binding.fencing_version = attempt.fencing_version
    UNION ALL
    SELECT candidate.sending_at, candidate.id IN (SELECT id FROM proven_unsent), false, NULL
    FROM obligation_deliveries AS candidate
    WHERE candidate.sending_at IS NOT NULL AND candidate.current_fence IS NULL
    UNION ALL
    SELECT quarantine.sent_at, false, false, NULL
    FROM obligation_deliveries AS candidate
    JOIN public.delivery_external_quarantines AS quarantine
      ON quarantine.delivery_id = candidate.id
    UNION ALL
    SELECT NULL::timestamptz, false, false, NULL
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
),
facts AS (
    SELECT
        (SELECT count(*) FROM returns WHERE returns.current_return) AS current_returns,
        (SELECT count(*) FROM returns) AS return_count,
        (SELECT count(*) FROM sends WHERE NOT sends.unsent) AS send_count,
        (SELECT count(*) FROM sends
         WHERE NOT sends.unsent AND (NOT sends.frozen OR sends.mid IS NULL)) AS sends_without_mid,
        (SELECT count(DISTINCT sends.mid) FROM sends
         WHERE NOT sends.unsent AND sends.mid IS NOT NULL) AS distinct_mids,
        EXISTS (
            SELECT 1
            FROM sends
            CROSS JOIN returns
            WHERE returns.current_return AND sends.sending_at >= returns.cutoff
        ) AS send_after_return,
        COALESCE(
            (SELECT min(sends.sending_at) FROM sends) < (SELECT min(returns.cutoff) FROM returns),
            false) AS first_send_before_return,
        EXISTS (
            SELECT 1
            FROM obligation_deliveries AS queued
            WHERE queued.status IN ('prepared', 'pending', 'claimed', 'sending')
              AND NOT (queued.current_dispatch
                       AND queued.status IN ('pending', 'claimed')
                       AND queued.sending_at IS NULL)
        ) AS in_flight
)
SELECT CASE
           WHEN current_returns <> 1 THEN 'return_not_current'
           WHEN in_flight THEN 'send_in_flight'
           WHEN send_count = return_count AND first_send_before_return
                AND NOT send_after_return THEN NULL
           WHEN sends_without_mid = 0 AND distinct_mids = send_count
                AND send_count = return_count AND first_send_before_return THEN NULL
           WHEN send_count > return_count AND sends_without_mid = 0 THEN 'sends_exceed_returns'
           WHEN send_count > return_count THEN 'send_without_sent_mid'
           WHEN send_count < return_count OR NOT first_send_before_return
               THEN 'send_records_incomplete'
           ELSE 'send_after_return'
       END AS blocker,
       CASE
           WHEN current_returns = 1 AND NOT in_flight AND send_count = return_count
                AND first_send_before_return AND NOT send_after_return THEN 'count'
           WHEN current_returns = 1 AND NOT in_flight AND sends_without_mid = 0
                AND distinct_mids = send_count AND send_count = return_count
                AND first_send_before_return THEN 'sent_mid'
       END AS rule
FROM facts"#
);

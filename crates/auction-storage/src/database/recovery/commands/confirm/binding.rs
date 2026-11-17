//! 确认端把恢复记录当前的退件与 Take 绑定到被收回的那次发送：原投递在退件之后不得再有发送尝试。
//!
//! 一封邮件只会被退回一次，被收回的必然是退件之前开始的发送。退件之后的新发送（旧版本 Admin
//! 缺口的重试、恢复 owner 之外的重新激活）产生的是另一封邮件，它的结果与旧退件、旧 Take 证明无关；
//! 确认若与之拼接，会在新发送的 fence 上追加收回处置、收口付款或交付并重列装备，而装备或载体可能
//! 正在新邮件里。绑定只读持久事实，不写任何状态。

use auction_application::recovery::RecoveryCommandError;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::commands::mapping::map_sql_error;

/// 原投递 `$1` 的发送都早于恢复记录 `$2` 当前退件的截止时刻。截止取两者中较早的一个：恢复记录
/// 消费这封退件的时刻，以及退件收件时刻加 3 分钟（收件箱时间只到分钟，再留出与退件时间窗相同的
/// 2 分钟远端时钟余量）。被退回的发送一定早于退件到达，也就早于两者；任一冻结发送尝试或当前
/// 发送围栏时刻不早于截止，都是退件之后的新发送。恢复记录、退件或消费事实缺失时为假。
const SENDS_PRECEDE_RETURN: &str = r#"SELECT COALESCE((
    SELECT NOT EXISTS (
               SELECT 1
               FROM public.delivery_send_attempts AS attempt
               WHERE attempt.delivery_id = delivery.id
                 AND attempt.sending_at >= bound.cutoff
           )
       AND (delivery.sending_at IS NULL OR delivery.sending_at < bound.cutoff)
    FROM public.deliveries AS delivery
    CROSS JOIN LATERAL (
        SELECT LEAST(consumed.recorded_at, returned.received_at + interval '3 minutes') AS cutoff
        FROM public.settlement_recoveries AS recovery
        JOIN public.inbound_mail AS returned ON returned.id = recovery.return_mail_id
        JOIN public.settlement_recovery_return_consumptions AS consumed
          ON consumed.return_mail_id = returned.id AND consumed.recovery_id = recovery.id
        WHERE recovery.id = $2
    ) AS bound
    WHERE delivery.id = $1
), false)"#;

/// 原投递的全部发送都早于恢复记录当前退件时返回真；调用方已锁定恢复记录与原投递，退件之后
/// 有新发送时拒绝确认（不变量 2、4、6）。
pub(super) async fn sends_precede_current_return(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    recovery_id: Uuid,
) -> Result<bool, RecoveryCommandError> {
    sqlx::query_scalar(SENDS_PRECEDE_RETURN)
        .bind(delivery_id)
        .bind(recovery_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)
}

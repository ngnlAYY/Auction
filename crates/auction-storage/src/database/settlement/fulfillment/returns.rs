//! 恢复流程不处理的结算退件的批处理入口：只负责开 Serializable 事务、按收件顺序选择一封尚未
//! 消费的入站退件，并把同一事务交给 `return_case_intake` 的写入链。证据、案件与履约迁移全部由
//! 该原语完成；批处理不领取、不退回、不重发，也不改变资金。
//!
//! 退件来源的 SQL 判定 `system_settlement_return_source!` 与应用层
//! `system_returned_settlement_obligation_id` 同一口径。恢复流程只消费满足它且绑定恢复记录付款或
//! 交付义务的退件，这里消费其余全部退件，两边分区互斥，同一封邮件只消费一次。退件按证据请求的
//! 派发（首寄或正文派发引用指向的补寄）绑定派发历史中的原投递，不默认绑定最新投递。

use auction_application::settlement::SettlementReturnConsumption;
use auction_time::Timestamp;
use sqlx::Postgres;
use uuid::Uuid;

use super::mapping::map_storage_error;
use super::return_case_intake::consume_return_for_mail_in_tx;

/// 入站退件来源可证明的 SQL 谓词（别名 `inbound` 为 `public.inbound_mail`）：持久化的发件人恰为
/// 系统 `MoogleMail`，主题按 ASCII 不区分大小写等于 `[RTS] ` + 证据义务 ID 的 canonical 结算主题。
/// 缺少发件人或主题的历史行为假，不会因 NULL 落入两个分区之外。
macro_rules! system_settlement_return_source {
    () => {
        r#"COALESCE(
               inbound.sender = 'MoogleMail'
               AND translate(inbound.subject, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ',
                             'abcdefghijklmnopqrstuvwxyz') COLLATE "C" IN (
                   '[rts] auction equipment delivery ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']',
                   '[rts] auction payment request ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']',
                   '[rts] auction seller payout ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']',
                   '[rts] auction buyer refund ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']'),
               false)"#
    };
}
pub(crate) use system_settlement_return_source;

/// 系统 RTS 且证据义务属于某条恢复记录的付款或交割义务；结算退件与恢复退件的唯一分区谓词。
macro_rules! recovery_owned_settlement_return {
    ($source:expr) => {
        concat!(
            $source,
            r#" AND EXISTS (
                 SELECT 1 FROM public.settlement_recoveries AS recovery
                 WHERE inbound.decision_metadata ->> 'obligation_id' IN (
                     recovery.payment_obligation_id::text,
                     recovery.equipment_obligation_id::text
                 )
             )"#
        )
    };
}
pub(crate) use recovery_owned_settlement_return;

/// 消费一封尚未消费的结算退件；没有时返回 `None`。单次 Serializable 事务，失败由 Worker 下一轮
/// 重试：无论证据是否可信或可绑定都会消费，不留下每轮重选的队头。
pub(crate) async fn consume_return_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
) -> Result<Option<SettlementReturnConsumption>, auction_application::core::RepositoryError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_storage_error)?;
    let Some(candidate) = sqlx::query(concat!(
        r#"SELECT inbound.id AS return_mail_id
           FROM public.inbound_mail AS inbound
           WHERE inbound.mail_kind = 'delivery'
             AND inbound.status = 'accepted'
             AND inbound.decision_code = 'settlement_recovery_returned'
             AND inbound.received_at <= $1
             AND NOT EXISTS (
                 SELECT 1 FROM public.settlement_recovery_return_consumptions AS consumed
                 WHERE consumed.return_mail_id = inbound.id
             )
             AND NOT EXISTS (
                 SELECT 1 FROM public.settlement_return_evidence AS evidence
                 WHERE evidence.return_mail_id = inbound.id
             )
             AND NOT ("#,
        recovery_owned_settlement_return!(system_settlement_return_source!()),
        r#")
           ORDER BY inbound.received_at, inbound.id
           LIMIT 1"#
    ))
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };
    let return_mail_id: Uuid =
        sqlx::Row::try_get(&candidate, "return_mail_id").map_err(map_storage_error)?;
    let consumption = consume_return_for_mail_in_tx(&mut transaction, return_mail_id, now).await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(consumption))
}

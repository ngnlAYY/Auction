//! 原派发冻结身份的锁内复验（D9 §4.2 第 1 步）：prepare / activate 摘要、正文摘要与冻结装备。
//!
//! 保护不变量 6：命令带的冻结身份必须逐项等于持久事实——投递行的 prepare 与 activate 摘要、派发轮与 Send
//! 尝试冻结的正文摘要与装备集合；Send 尝试的期望还要与派发轮、投递行互相一致。任一项不符都按冲突整笔
//! 拒绝，COD 收据不会绑定到同一 delivery 的另一份正文或另一件装备上。

use auction_application::settlement::{CertifyCod, FundingError};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage;

/// 逐项复验冻结身份；`attempt_id` 是已由 `load_facts` 绑定的原 Send 尝试。
pub(super) async fn verify_freeze(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    attempt_id: Uuid,
) -> Result<(), FundingError> {
    let freeze = &command.freeze;
    let row = sqlx::query(
        r#"SELECT delivery.request_digest = $2 AS prepare_ok,
                  delivery.activation_command_id IS NOT NULL
                      AND delivery.activation_request_digest = $3 AS activation_ok,
                  round.body_digest = $4
                      AND send.expectation->'body_digest'
                          = (SELECT jsonb_agg(get_byte($4, position) ORDER BY position)
                             FROM generate_series(0, 31) AS position) AS body_ok,
                  send.request_digest = delivery.request_digest
                      AND send.expectation->>'recipient' = delivery.recipient
                      AND send.expectation->>'subject' = round.subject
                      AND send.expectation->>'world_id' = round.world_id
                      AND (send.expectation->>'credits')::bigint = round.credits
                      AND (send.expectation->>'cod')::bigint = round.cod AS send_ok,
                  COALESCE(delivery.payload->'equipment_ids', '[]'::jsonb) = to_jsonb($5::text[])
                      AND send.expectation->'equipment_ids' = to_jsonb($5::text[]) AS equipment_ok
           FROM public.consignment_dispatch_attempts AS attempt
           JOIN public.consignment_dispatch_rounds AS round
             ON round.contract_id = attempt.contract_id
            AND round.obligation_id = attempt.obligation_id
            AND round.round = attempt.round AND round.delivery_id = attempt.delivery_id
           JOIN public.deliveries AS delivery ON delivery.id = attempt.delivery_id
           JOIN public.delivery_send_attempts AS send
             ON send.delivery_id = attempt.delivery_id AND send.fencing_version = attempt.fence
           WHERE attempt.attempt_id = $1"#,
    )
    .bind(attempt_id)
    .bind(freeze.prepare_digest.as_slice())
    .bind(freeze.activation_digest.as_slice())
    .bind(freeze.body_digest.as_slice())
    .bind(&freeze.equipment_ids)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    for column in ["prepare_ok", "activation_ok", "body_ok", "send_ok", "equipment_ok"] {
        // 比较结果为 NULL（缺列或缺值）同样不是通过。
        if !row.try_get::<Option<bool>, _>(column).map_err(storage)?.unwrap_or(false) {
            return Err(FundingError::Conflict);
        }
    }
    Ok(())
}

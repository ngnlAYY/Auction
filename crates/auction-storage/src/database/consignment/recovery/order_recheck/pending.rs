//! 订单复核的待核对项：已起写、无终局证据的派发尝试与 Take，以及完成时对领取快照的终局核对。
//!
//! 终局证据的判据与 000137 ⑳ 的结果守卫 `c7_guard_order_recheck_result` 一致：派发尝试以
//! `delivery_sent_bindings` 的 Sent 绑定为终局，Take 以结论性证明（`take_proven`、`return_proven`）为终局；
//! `evidence_quarantined` 不是终局。这里只读，不写任何证据或起写事实（不变量 4）。

use auction_application::consignment::{RecheckDispatchAttempt, RecheckReturnTake};
use auction_application::delivery::{DeliverySendCredentialBinding, DeliverySendExpectation};
use auction_domain::consignment::returns::ReturnLeg;
use auction_domain::consignment::{OriginalMailKey, parse_settlement_world};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::invariant_decoding;

/// 合同上已起写、没有 Sent 绑定的派发尝试，按起写时刻排序。
pub(super) async fn pending_dispatch_attempts(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<RecheckDispatchAttempt>, WriteError> {
    let rows = sqlx::query(
        "SELECT attempt.attempt_id, attempt.delivery_id, attempt.fence, attempt.obligation_id,
                attempt.round, attempt.leg, attempt.started_at, round_row.round_id,
                round_row.bot_uid, round_row.world_id, send.sending_at, send.expectation
         FROM public.consignment_dispatch_attempts AS attempt
         JOIN public.consignment_dispatch_rounds AS round_row
           ON round_row.contract_id = attempt.contract_id
          AND round_row.obligation_id = attempt.obligation_id
          AND round_row.round = attempt.round
          AND round_row.delivery_id = attempt.delivery_id
         JOIN public.delivery_send_attempts AS send
           ON send.delivery_id = attempt.delivery_id AND send.fencing_version = attempt.fence
         WHERE attempt.contract_id = $1
           AND NOT EXISTS (
               SELECT 1 FROM public.delivery_sent_bindings AS sent
               WHERE sent.delivery_id = attempt.delivery_id
                 AND sent.fencing_version = attempt.fence)
         ORDER BY attempt.started_at, attempt.attempt_id",
    )
    .bind(contract_id)
    .fetch_all(connection)
    .await?;
    rows.iter().map(dispatch_attempt).collect()
}

fn dispatch_attempt(row: &PgRow) -> Result<RecheckDispatchAttempt, WriteError> {
    let world = parse_settlement_world(row.try_get::<&str, _>("world_id")?)
        .ok_or_else(|| WriteError::invariant("consignment_recheck_world"))?;
    // 冻结 Send 身份由起写事务写入；形状不符说明事实被改坏，失败关闭而不是猜测核对目标。
    let expectation: DeliverySendExpectation = serde_json::from_value(row.try_get("expectation")?)
        .map_err(invariant_decoding("consignment_recheck_expectation"))?;
    Ok(RecheckDispatchAttempt {
        attempt_id: row.try_get("attempt_id")?,
        round_id: row.try_get("round_id")?,
        obligation_id: row.try_get("obligation_id")?,
        round: u32::try_from(row.try_get::<i32, _>("round")?)?,
        leg: row.try_get("leg")?,
        delivery_id: row.try_get("delivery_id")?,
        fence: u64::try_from(row.try_get::<i64, _>("fence")?)?,
        bot_uid: row.try_get("bot_uid")?,
        world,
        expectation,
        sending_at: row.try_get("sending_at")?,
        started_at: row.try_get("started_at")?,
    })
}

/// 合同上已起写、没有结论性证明的 Take（只看退件接管建立的唯一 Take 工作），按起写时刻排序。
pub(super) async fn pending_return_takes(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<RecheckReturnTake>, WriteError> {
    let rows = sqlx::query(
        "SELECT intake.id AS intake_id, intake.decision_id, intake.leg, intake.bot_uid,
                intake.world_id, intake.external_mid, started.work_id, started.delivery_id,
                started.attempt_id, started.fence, started.started_at, attempt.credential_id,
                attempt.credential_version, attempt.runtime_generation, attempt.session_id,
                attempt.expected_digest, work.manifest_digest
         FROM public.consignment_return_intakes AS intake
         JOIN public.mail_disposition_write_starts AS started
           ON started.work_id = intake.take_work_id
          AND started.delivery_id = intake.take_delivery_id
         JOIN public.mail_disposition_attempts AS attempt
           ON attempt.id = started.attempt_id AND attempt.work_id = started.work_id
          AND attempt.fence = started.fence
         JOIN public.mail_disposition_work AS work ON work.id = started.work_id
         WHERE intake.contract_id = $1
           AND NOT EXISTS (
               SELECT 1 FROM public.mail_disposition_proofs AS proof
               WHERE proof.work_id = started.work_id
                 AND proof.conclusion IN ('take_proven', 'return_proven'))
         ORDER BY started.started_at, started.work_id",
    )
    .bind(contract_id)
    .fetch_all(connection)
    .await?;
    rows.iter().map(return_take).collect()
}

fn digest(row: &PgRow, column: &'static str) -> Result<[u8; 32], WriteError> {
    <[u8; 32]>::try_from(row.try_get::<&[u8], _>(column)?)
        .map_err(invariant_decoding("consignment_recheck_digest"))
}

fn return_take(row: &PgRow) -> Result<RecheckReturnTake, WriteError> {
    let world = parse_settlement_world(row.try_get::<&str, _>("world_id")?)
        .ok_or_else(|| WriteError::invariant("consignment_recheck_world"))?;
    let mail_key = OriginalMailKey::new(
        row.try_get::<&str, _>("bot_uid")?,
        world,
        row.try_get::<&str, _>("external_mid")?,
    )
    .map_err(invariant_decoding("consignment_recheck_mail_key"))?;
    let leg = ReturnLeg::parse(row.try_get::<&str, _>("leg")?)
        .map_err(invariant_decoding("consignment_recheck_leg"))?;
    Ok(RecheckReturnTake {
        intake_id: row.try_get("intake_id")?,
        decision_id: row.try_get("decision_id")?,
        leg,
        mail_key,
        work_id: row.try_get("work_id")?,
        delivery_id: row.try_get("delivery_id")?,
        attempt_id: row.try_get("attempt_id")?,
        fence: u64::try_from(row.try_get::<i64, _>("fence")?)?,
        credential: DeliverySendCredentialBinding {
            credential_id: row.try_get("credential_id")?,
            credential_version: row.try_get("credential_version")?,
            runtime_generation: row.try_get("runtime_generation")?,
        },
        session_id: row.try_get("session_id")?,
        expected_digest: digest(row, "expected_digest")?,
        manifest_digest: digest(row, "manifest_digest")?,
        started_at: row.try_get("started_at")?,
    })
}

/// 领取快照里是否仍有一项没有终局证据；判据与结果守卫逐字同义。
pub(super) async fn snapshot_has_open_items(
    connection: &mut PgConnection,
    dispatch_attempts: &[Uuid],
    take_works: &[Uuid],
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
                    SELECT 1 FROM public.consignment_dispatch_attempts AS attempt
                    WHERE attempt.attempt_id = ANY ($1)
                      AND NOT EXISTS (
                          SELECT 1 FROM public.delivery_sent_bindings AS sent
                          WHERE sent.delivery_id = attempt.delivery_id
                            AND sent.fencing_version = attempt.fence))
                OR EXISTS (
                    SELECT 1 FROM unnest($2::uuid[]) AS item(take_work_id)
                    WHERE NOT EXISTS (
                        SELECT 1 FROM public.mail_disposition_proofs AS proof
                        WHERE proof.work_id = item.take_work_id
                          AND proof.conclusion IN ('take_proven', 'return_proven')))",
    )
    .bind(dispatch_attempts)
    .bind(take_works)
    .fetch_one(connection)
    .await?)
}

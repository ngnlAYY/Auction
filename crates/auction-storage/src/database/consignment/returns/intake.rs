//! 退件接管的锁内读取：原发送来源、既有接管、终局槽与同轮同腿占用。
//!
//! 全部读取都在调用方已持有的接管事务内执行（命令 → 买家用户 → 规范实物 → K 之后），结果只喂给
//! 领域 `decide_acquire_return`；本模块不做业务取舍，读不出的事实一律返回空，让领域判定隔离。

use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_domain::consignment::returns::{
    ExistingIntake, KFinalDisposition, ReturnIntakeFacts, ReturnLeg, RtsOrigin,
};
use auction_domain::consignment::settlement_world_code;
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::identity::{MailKeyColumns, parse_world};
use crate::database::mail::originals::invariant_decoding;
use crate::database::mail::originals::observations::StoredObservation;

/// 如何定位被退回的那一轮。
#[derive(Debug, Clone, Copy)]
pub(crate) enum RoundLocator {
    /// 应用层从 RTS 主题证明的原义务与原轮。
    ByObligationRound { obligation_id: Uuid, round: u32 },
    /// 隔离解除：只有 RTS 正文末行的派发引用（轮 ID）。
    ByRoundId(Uuid),
}

/// 锁内读出的来源，连同该轮冻结的 Credits（现金腿的阻断金额）。
pub(super) struct OriginRead {
    pub(super) origin: RtsOrigin,
    /// RTS 清单与原发送冻结身份逐项一致时才有值；它就是 Take 工作要冻结的清单摘要。
    pub(super) observed_manifest_digest: Option<[u8; 32]>,
}

/// 已落库的接管行。
#[derive(Debug, Clone)]
pub(super) struct IntakeRow {
    pub(super) intake_id: Uuid,
    pub(super) key: OriginalMailKey,
    pub(super) decision_id: Uuid,
    pub(super) take_work_id: Uuid,
    pub(super) take_delivery_id: Uuid,
    pub(super) facts: ReturnIntakeFacts,
}

/// 接管行的全部列；拼接成字面量 SQL，避免动态字符串。
macro_rules! intake_select {
    ($tail:literal) => {
        concat!(
            "SELECT id, bot_uid, world_id, external_mid, decision_id, contract_id, obligation_id,
                original_round, original_delivery_id, original_fence, sent_mid, leg,
                rts_observation_id, rts_returned_at, manifest_digest, session_id, take_work_id,
                take_delivery_id
             FROM public.consignment_return_intakes ",
            $tail
        )
    };
}

fn digest(bytes: &[u8], reason: &'static str) -> Result<[u8; 32], WriteError> {
    <[u8; 32]>::try_from(bytes).map_err(invariant_decoding(reason))
}

fn row_to_intake(row: &sqlx::postgres::PgRow) -> Result<IntakeRow, WriteError> {
    let world: String = row.try_get("world_id")?;
    let key = OriginalMailKey::new(
        &row.try_get::<String, _>("bot_uid")?,
        parse_world(&world)?,
        &row.try_get::<String, _>("external_mid")?,
    )
    .map_err(invariant_decoding("consignment_intake_mail_key"))?;
    let leg: String = row.try_get("leg")?;
    let facts = ReturnIntakeFacts {
        k: key.clone(),
        contract_id: row.try_get("contract_id")?,
        obligation_id: row.try_get("obligation_id")?,
        original_round: u32::try_from(row.try_get::<i32, _>("original_round")?)?,
        original_attempt: AttemptKey {
            delivery_id: row.try_get("original_delivery_id")?,
            fence: u64::try_from(row.try_get::<i64, _>("original_fence")?)?,
        },
        sent_mid: row.try_get("sent_mid")?,
        rts_observation_id: row.try_get("rts_observation_id")?,
        rts_returned_at: row.try_get::<Timestamp, _>("rts_returned_at")?,
        leg: ReturnLeg::parse(&leg).map_err(invariant_decoding("consignment_intake_leg"))?,
        frozen_manifest_digest: digest(
            row.try_get::<&[u8], _>("manifest_digest")?,
            "consignment_intake_manifest",
        )?,
        session_id: row.try_get("session_id")?,
    };
    Ok(IntakeRow {
        intake_id: row.try_get("id")?,
        key,
        decision_id: row.try_get("decision_id")?,
        take_work_id: row.try_get("take_work_id")?,
        take_delivery_id: row.try_get("take_delivery_id")?,
        facts,
    })
}

/// 按 K 读接管；不加锁（调用方已持 K 的会话锁）。
pub(super) async fn load_intake_by_key(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
) -> Result<Option<IntakeRow>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let row =
        sqlx::query(intake_select!("WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3"))
            .bind(columns.bot_uid)
            .bind(columns.world_id)
            .bind(columns.external_mid)
            .fetch_optional(&mut *connection)
            .await?;
    row.as_ref().map(row_to_intake).transpose()
}

/// 按 ID 读接管；接管行只追加且运行账号对它没有 UPDATE 权限，不能加行锁，一致性由 K 的会话锁保证。
pub(super) async fn load_intake(
    connection: &mut PgConnection,
    intake_id: Uuid,
) -> Result<Option<IntakeRow>, WriteError> {
    let row = sqlx::query(intake_select!("WHERE id = $1"))
        .bind(intake_id)
        .fetch_optional(&mut *connection)
        .await?;
    row.as_ref().map(row_to_intake).transpose()
}

/// 接管行转领域的既有接管。
pub(super) fn existing_of(row: &IntakeRow) -> ExistingIntake {
    ExistingIntake {
        contract_id: row.facts.contract_id,
        obligation_id: row.facts.obligation_id,
        original_round: row.facts.original_round,
        original_attempt: row.facts.original_attempt,
        sent_mid: row.facts.sent_mid.clone(),
        leg: row.facts.leg,
        frozen_manifest_digest: row.facts.frozen_manifest_digest,
    }
}

/// 该 K 在终局槽里的结论。
pub(super) async fn k_final(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
) -> Result<KFinalDisposition, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM public.mail_final_dispositions
         WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(match kind.as_deref() {
        None => KFinalDisposition::None,
        Some("acquire_return") => KFinalDisposition::AcquireReturn,
        Some(_) => KFinalDisposition::Other,
    })
}

/// 同轮同腿已被另一个 K 接管（`UNIQUE(合同, 义务, 原轮, 腿)` 的软前置，唯一键兜底）。
pub(super) async fn same_leg_other_k(
    connection: &mut PgConnection,
    origin: &RtsOrigin,
    key: &OriginalMailKey,
) -> Result<Option<(Uuid, OriginalMailKey)>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let row = sqlx::query(
        "SELECT id, bot_uid, world_id, external_mid FROM public.consignment_return_intakes
         WHERE contract_id = $1 AND obligation_id = $2 AND original_round = $3 AND leg = $4
           AND NOT (bot_uid = $5 AND world_id = $6 AND external_mid = $7)",
    )
    .bind(origin.contract_id)
    .bind(origin.obligation_id)
    .bind(i32::try_from(origin.original_round)?)
    .bind(origin.leg.as_str())
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(|row| {
        let world: String = row.try_get("world_id")?;
        let other = OriginalMailKey::new(
            &row.try_get::<String, _>("bot_uid")?,
            parse_world(&world)?,
            &row.try_get::<String, _>("external_mid")?,
        )
        .map_err(invariant_decoding("consignment_intake_mail_key"))?;
        Ok((row.try_get("id")?, other))
    })
    .transpose()
}

fn leg_of_attempt(value: &str) -> Option<ReturnLeg> {
    // 寄售的 isekai 载体派发只寄 1 Credit（COD P+1、无附件），被退回的是载体本金，不是装备：
    // `carrier` 尝试映射为现金本金腿 `CashPrincipal`，Take 起写同事务开该 UID 的 opening 阻断，消费时
    // 经 `return_cash_in_tx` 回补原 carrier 并记 `rts_principal`（D9 §4.2、§4.4；统筹方 PIWS5 裁定 4）。
    // 映射成装备腿会漏掉阻断与回补，失败方向是资金责任悬空（不变量 4、6）。
    match value {
        "original" => Some(ReturnLeg::Original),
        "carrier" => Some(ReturnLeg::CashPrincipal),
        "cash_payout" => Some(ReturnLeg::CashPayout),
        "cash_refund" => Some(ReturnLeg::CashRefund),
        _ => None,
    }
}

/// 由派发轮、唯一的 Sent 绑定与冻结尝试得出 RTS 的来源；任一环读不出或不唯一就返回空。
///
/// 保护不变量 4、6：RTS 只有在原发送的合同、义务、轮、Sent MID、腿与冻结身份全部由本地持久
/// 事实指认时才有来源；清单摘要只在 RTS 的 Credits、COD 与装备集合和冻结发送身份逐项一致时
/// 才作为“已验证”交给领域（它同时是 Take 工作的冻结清单）。
pub(super) async fn read_origin(
    connection: &mut PgConnection,
    locator: RoundLocator,
    key: &OriginalMailKey,
    observation: &StoredObservation,
) -> Result<Option<OriginRead>, WriteError> {
    let round =
        match locator {
            RoundLocator::ByObligationRound { obligation_id, round } => sqlx::query(
                "SELECT contract_id, obligation_id, round, round_id, delivery_id, bot_uid, world_id
                 FROM public.consignment_dispatch_rounds WHERE obligation_id = $1 AND round = $2",
            )
            .bind(obligation_id)
            .bind(i32::try_from(round)?)
            .fetch_optional(&mut *connection)
            .await?,
            RoundLocator::ByRoundId(round_id) => sqlx::query(
                "SELECT contract_id, obligation_id, round, round_id, delivery_id, bot_uid, world_id
                 FROM public.consignment_dispatch_rounds WHERE round_id = $1",
            )
            .bind(round_id)
            .fetch_optional(&mut *connection)
            .await?,
        };
    let Some(round) = round else { return Ok(None) };
    let round_id: Uuid = round.try_get("round_id")?;
    // 正文末行的派发引用若存在，必须就是这一轮；不符说明 RTS 指向别的发送。
    if observation
        .legacy_return_evidence
        .dispatch_reference
        .as_deref()
        .is_some_and(|reference| reference != round_id.to_string())
    {
        return Ok(None);
    }
    let delivery_id: Uuid = round.try_get("delivery_id")?;
    let bindings = sqlx::query(
        "SELECT binding.external_mail_id, binding.fencing_version, attempt.sending_at,
                attempt.expectation
         FROM public.delivery_sent_bindings AS binding
         JOIN public.delivery_send_attempts AS attempt
           ON attempt.delivery_id = binding.delivery_id
          AND attempt.fencing_version = binding.fencing_version
         WHERE binding.delivery_id = $1 AND binding.world_id = $2",
    )
    .bind(delivery_id)
    .bind(settlement_world_code(key.world()))
    .fetch_all(&mut *connection)
    .await?;
    // 同一轮投递在该世界恰有一个已绑定的 Sent MID，才能指认“被退回的是哪一封”。
    let [binding] = bindings.as_slice() else { return Ok(None) };
    let fence = u64::try_from(binding.try_get::<i64, _>("fencing_version")?)?;
    let leg: Option<String> = sqlx::query_scalar(
        "SELECT leg FROM public.consignment_dispatch_attempts
         WHERE delivery_id = $1 AND fence = $2",
    )
    .bind(delivery_id)
    .bind(i64::try_from(fence)?)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(leg) = leg.as_deref().and_then(leg_of_attempt) else { return Ok(None) };
    let expectation: Value = binding.try_get("expectation")?;
    let manifest_matches = frozen_manifest_matches(&expectation, observation);
    let world: String = round.try_get("world_id")?;
    Ok(Some(OriginRead {
        origin: RtsOrigin {
            contract_id: round.try_get("contract_id")?,
            obligation_id: round.try_get("obligation_id")?,
            original_round: u32::try_from(round.try_get::<i32, _>("round")?)?,
            original_attempt: AttemptKey { delivery_id, fence },
            bot_uid: round.try_get("bot_uid")?,
            world: parse_world(&world)?,
            sent_mid: binding.try_get("external_mail_id")?,
            sent_at: binding.try_get::<Timestamp, _>("sending_at")?,
            leg,
            frozen_manifest_digest: observation.manifest_digest,
            session_id: observation.session_id,
        },
        observed_manifest_digest: manifest_matches.then_some(observation.manifest_digest),
    }))
}

/// RTS 的 Credits、COD 与装备集合必须与冻结的发送身份一致；任何一项读不出都不一致。
fn frozen_manifest_matches(expectation: &Value, observation: &StoredObservation) -> bool {
    let credits = expectation.get("credits").and_then(Value::as_i64);
    let cod = expectation.get("cod").and_then(Value::as_i64);
    let Some(frozen_ids) = expectation.get("equipment_ids").and_then(Value::as_array) else {
        return false;
    };
    let mut frozen: Vec<&str> = frozen_ids.iter().filter_map(Value::as_str).collect();
    let mut observed: Vec<&str> =
        observation.items.iter().filter_map(|item| item.canonical_eid.as_deref()).collect();
    if frozen.len() != frozen_ids.len() || observed.len() != observation.items.len() {
        return false;
    }
    frozen.sort_unstable();
    observed.sort_unstable();
    credits.is_some()
        && cod.is_some()
        && credits == observation.credits
        && cod == observation.cod
        && frozen == observed
}

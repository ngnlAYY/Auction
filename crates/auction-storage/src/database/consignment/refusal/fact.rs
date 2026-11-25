//! 返回事实：从退件接管行与合同 Sale 读取事实身份，幂等保存不可变的返回事实。
//!
//! 保护不变量 1、6：返回事实 ID 由 (K, 原轮, 资产腿) 派生，同 K 重复 RTS 落到同一行、不多计；
//! 同 ID 但身份列与已存事实不一致一律失败关闭，不覆盖、不静默修正。

use auction_domain::UserId;
use auction_domain::consignment::refusal::{RefusalKind, return_fact_id};
use auction_domain::consignment::{OriginalMailKey, parse_settlement_world};
use auction_time::{Duration, Timestamp};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 退件接管行加合同 Sale 给出的事实身份与冻结阈值。
pub(super) struct IntakeContext {
    pub(super) key: OriginalMailKey,
    pub(super) contract_id: Uuid,
    pub(super) obligation_id: Uuid,
    pub(super) original_round: u32,
    pub(super) leg: String,
    pub(super) returned_at: Timestamp,
    pub(super) buyer: UserId,
    /// 合同世界对应的拒收种类。
    pub(super) world_kind: RefusalKind,
    /// 触发合同冻结的阈值：main 取 `cod_return_limit`，isekai 取 `isekai_refusal_limit`。
    pub(super) threshold: u8,
}

/// 已存事实的列：接管行、合同、义务、买家、种类、周期与四个时刻。
type FactRow =
    (Uuid, Uuid, Uuid, Uuid, String, Uuid, Timestamp, Timestamp, Timestamp, Option<Timestamp>);

type IntakeRow =
    (String, String, String, Uuid, Uuid, i32, String, Timestamp, Uuid, i16, i16, String);

/// 读取退件接管行及其合同 Sale；行不存在或值越界一律失败关闭。
pub(super) async fn load_intake_context(
    conn: &mut PgConnection,
    intake_id: Uuid,
) -> Result<IntakeContext, WriteError> {
    let row: Option<IntakeRow> = sqlx::query_as(
        "SELECT intake.bot_uid, intake.world_id, intake.external_mid, intake.contract_id,
                intake.obligation_id, intake.original_round, intake.leg, intake.rts_returned_at,
                sale.buyer_user_id, sale.cod_return_limit, sale.isekai_refusal_limit,
                contract.world_id
         FROM public.consignment_return_intakes AS intake
         JOIN public.consignment_contracts AS contract ON contract.contract_id = intake.contract_id
         JOIN public.consignment_sales AS sale ON sale.contract_id = intake.contract_id
         WHERE intake.id = $1",
    )
    .bind(intake_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((
        bot,
        mail_world,
        mid,
        contract_id,
        obligation_id,
        round,
        leg,
        returned_at,
        buyer,
        cod,
        isekai,
        contract_world,
    )) = row
    else {
        return Err(WriteError::invariant("consignment_refusal_intake_missing"));
    };
    let mail_world = parse_settlement_world(&mail_world)
        .ok_or_else(|| WriteError::invariant("consignment_refusal_world"))?;
    let key = OriginalMailKey::new(&bot, mail_world, &mid).map_err(|error| {
        tracing::warn!(%error, "退件接管行的原信键无效");
        WriteError::invariant("consignment_refusal_mail_key")
    })?;
    let (world_kind, threshold) = match contract_world.as_str() {
        "main" => (RefusalKind::MainCodReturn, cod),
        "isekai" => (RefusalKind::IsekaiUnpaidCarrierReturn, isekai),
        _ => return Err(WriteError::invariant("consignment_refusal_world")),
    };
    Ok(IntakeContext {
        key,
        contract_id,
        obligation_id,
        original_round: u32::try_from(round)?,
        leg,
        returned_at,
        buyer: UserId::from_uuid(buyer),
        world_kind,
        threshold: u8::try_from(threshold)?,
    })
}

/// 已保存的返回事实；不可变，重裁决只追加裁决行。
pub(super) struct StoredFact {
    pub(super) return_fact_id: Uuid,
    pub(super) cycle_id: Uuid,
    pub(super) returned_at: Timestamp,
    pub(super) returned_minute_end: Timestamp,
    pub(super) proven_at: Timestamp,
    pub(super) delivery_close_at: Option<Timestamp>,
}

/// 拒收种类的持久文本。
pub(super) const fn kind_text(kind: RefusalKind) -> &'static str {
    match kind {
        RefusalKind::MainCodReturn => "main_cod_return",
        RefusalKind::IsekaiUnpaidCarrierReturn => "isekai_unpaid_carrier_return",
    }
}

/// 事实身份：(K, 原轮, 腿) 派生的事实 ID。
pub(super) fn fact_id_of(context: &IntakeContext) -> Uuid {
    return_fact_id(&context.key, context.original_round, &context.leg)
}

/// 读取已存事实并核对身份；不存在返回 `None`。
pub(super) async fn load_stored_fact(
    conn: &mut PgConnection,
    context: &IntakeContext,
    intake_id: Uuid,
    kind: RefusalKind,
) -> Result<Option<StoredFact>, WriteError> {
    let fact_id = fact_id_of(context);
    let row: Option<FactRow> = sqlx::query_as(
        "SELECT intake_id, contract_id, obligation_id, buyer_user_id, kind, cycle_id,
                    returned_at, returned_minute_end, proven_at, delivery_close_at
             FROM public.consignment_return_facts WHERE return_fact_id = $1",
    )
    .bind(fact_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((
        stored_intake,
        contract_id,
        obligation_id,
        buyer,
        stored_kind,
        cycle_id,
        returned_at,
        returned_minute_end,
        proven_at,
        delivery_close_at,
    )) = row
    else {
        return Ok(None);
    };
    // 同 ID 却身份不同：绝不覆盖也不凑数（不变量 6）。
    if stored_intake != intake_id
        || contract_id != context.contract_id
        || obligation_id != context.obligation_id
        || buyer != context.buyer.as_uuid()
        || stored_kind != kind_text(kind)
    {
        return Err(WriteError::invariant("consignment_refusal_fact_conflict"));
    }
    Ok(Some(StoredFact {
        return_fact_id: fact_id,
        cycle_id,
        returned_at,
        returned_minute_end,
        proven_at,
        delivery_close_at,
    }))
}

/// 保存新的返回事实；`returned_minute_end` 恒为发生分钟起点加一分钟（表约束同口径）。
pub(super) async fn insert_fact(
    conn: &mut PgConnection,
    context: &IntakeContext,
    intake_id: Uuid,
    kind: RefusalKind,
    cycle_id: Uuid,
    proven_at: Timestamp,
    delivery_close_at: Option<Timestamp>,
) -> Result<StoredFact, WriteError> {
    let return_fact_id = fact_id_of(context);
    let returned_minute_end = context
        .returned_at
        .checked_add_signed(Duration::minutes(1))
        .ok_or_else(|| WriteError::invariant("consignment_refusal_time_range"))?;
    let proven_at = proven_at.trunc_subsecs(6);
    let delivery_close_at = delivery_close_at.map(|close| close.trunc_subsecs(6));
    sqlx::query(
        "INSERT INTO public.consignment_return_facts
            (return_fact_id, intake_id, contract_id, obligation_id, original_round, leg, kind,
             buyer_user_id, cycle_id, returned_at, returned_minute_end, proven_at, delivery_close_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(return_fact_id)
    .bind(intake_id)
    .bind(context.contract_id)
    .bind(context.obligation_id)
    .bind(i32::try_from(context.original_round)?)
    .bind(&context.leg)
    .bind(kind_text(kind))
    .bind(context.buyer.as_uuid())
    .bind(cycle_id)
    .bind(context.returned_at)
    .bind(returned_minute_end)
    .bind(proven_at)
    .bind(delivery_close_at)
    .execute(&mut *conn)
    .await?;
    Ok(StoredFact {
        return_fact_id,
        cycle_id,
        returned_at: context.returned_at,
        returned_minute_end,
        proven_at,
        delivery_close_at,
    })
}

//! 补寄新轮的存储来源：已排队、尚未建轮的补寄处置读口，以及授权前为新轮备好 `prepared` 投递的写端。
//!
//! 保护不变量 4、5、6：
//! - 只处理 Admin 已排队的 `redispatch_original` 处置，新轮键 = 处置引用的已消费原件接管所封闭的
//!   那一轮加一，且其派生轮 ID 必须等于处置记下的 `dispatch_id`；上一轮之后已有更新的轮、义务不是
//!   未付 COD 原件腿（统筹方裁定的保守读法）或来源缺失时一律不列、不备。
//! - 新轮冻结身份全部在备好事务里确定：权利人 = 义务受益人（合同冻结买方），Bot UID、世界、Credits、
//!   COD 沿用上一轮，轮引用 = 新轮 ID，D0 = 备好时的数据库时刻 + 合同冻结的付款期限（维护者裁定，
//!   与首轮 D0 = 成交时刻 + 同一冻结期限同口径；原轮正文的 D0 与每合同一次的提醒键不变），正文经与
//!   购买同一冻结模板内容来源渲染。渲染出的模板、主题须与上一轮一致、附件须等于合同规范装备，D0 不得
//!   早于上一轮；任一不符失败关闭、不建投递。
//! - 投递与投递命令 ID 由新轮 ID 确定性派生，同一新轮至多一封备好的投递：已备好时重放返回原投递与原
//!   冻结身份，不重渲染、不覆盖。投递停在 `prepared`，通用调度不领取；建轮仍只经 `authorize_dispatch`，
//!   起写与 POST 仍只经派发 executor 与一次性许可。本写端不外写。

use std::sync::Arc;

use auction_application::consignment::{
    PreparedRedispatch, QueuedRedispatch, RedispatchPrepared, ScanCursor, ScanPage,
};
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref, dispatch_round_id,
};
use auction_domain::consignment::{ConsignmentObligation, ListingPrice, parse_settlement_world};
use auction_domain::{
    ConsignmentContractId, ConsignmentContractKind, SettlementAmount, SettlementId,
    SettlementObligationKind, SettlementObligationState, SettlementWorld, UserId,
};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::first_dispatch::{hex, payload_matches, round_delivery_ids, send_payload};
use super::round::{latest_round, load_by_key};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::purchase::{FirstDispatchContentSource, FirstDispatchRequest};
use crate::database::consignment::scan::{cursor_binds, full_page_cursor, row_cursor};
use crate::database::delivery::audit::{DeliveryAudit, PREPARE, append as append_delivery_audit};

/// 补寄只放行未付 COD 原件腿（D9 §5.1；统筹方裁定的保守读法）。
const UNPAID_ORIGINAL_KIND: &str = "winner_equipment_cod";

/// 一条已排队、尚未建轮的补寄处置及其封闭的上一轮。
struct QueuedRow {
    scan: ScanCursor,
    dispatch_id: Uuid,
    contract_id: Uuid,
    intake_id: Uuid,
    obligation_id: Uuid,
    obligation_kind: String,
    original_round: i32,
}

/// 新轮冻结身份中由存储确定的部分与渲染所需的合同事实。
struct Basis {
    key: DispatchRoundKey,
    rights_holder_identity: String,
    bot_uid: String,
    world: SettlementWorld,
    subject: String,
    template: String,
    round_ref: String,
    credits: i64,
    cod: i64,
    canonical_eid: String,
    prior_d0: Timestamp,
    item_name: String,
    price: i64,
    payment_deadline_days: i16,
}

impl DatabaseStore {
    /// 列出游标之后已排队、尚未建轮的补寄处置（见模块说明），按 `(resolved_at, dispatch_id)` 升序一页。
    /// 来源不符的行（不写任何行，下一轮仍会被选中）不列入但计入游标，异常行不会饿死后面的补寄；读满一页
    /// 才给出下一页游标。只读。
    pub(super) async fn load_queued_redispatches(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> Result<ScanPage<QueuedRedispatch>, WriteError> {
        let mut transaction = self.pool().begin_with("BEGIN READ ONLY").await?;
        let rows = queued_rows(&mut transaction, None, after, i64::from(limit)).await?;
        let next = full_page_cursor(rows.len(), limit, rows.last().map(|row| row.scan));
        let mut queued = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(basis) = basis_of(&mut transaction, &row).await? else {
                tracing::warn!(
                    code = "redispatch_source_mismatch",
                    "补寄处置的来源不符，不列入待办"
                );
                continue;
            };
            let prepared = prepared_of(&mut transaction, row.dispatch_id).await?;
            queued.push(QueuedRedispatch {
                dispatch_id: row.dispatch_id,
                key: basis.key,
                closed_by_return_intake: row.intake_id,
                prepared,
            });
        }
        transaction.commit().await?;
        Ok(ScanPage { items: queued, next })
    }

    /// 为已排队的补寄新轮备好 `prepared` 投递（见模块说明）。命令锁键取新轮 ID，使同一新轮的
    /// 并发备好者在会话锁上排队；一个顶层 Serializable 事务，序列化冲突有界重试。
    pub(super) async fn prepare_redispatch_delivery(
        &self,
        dispatch_id: Uuid,
        content: &Arc<dyn FirstDispatchContentSource>,
    ) -> Result<Option<RedispatchPrepared>, WriteError> {
        let keys = LockKeys { command_id: dispatch_id, sellers: &[], equipment: &[] };
        let input = (dispatch_id, Arc::clone(content));
        with_deposit_transaction(self, keys, &[], &input, |input, tx| {
            Box::pin(prepare_in_tx(input, tx))
        })
        .await
    }
}

async fn queued_rows(
    connection: &mut PgConnection,
    dispatch_id: Option<Uuid>,
    after: Option<ScanCursor>,
    limit: i64,
) -> Result<Vec<QueuedRow>, WriteError> {
    let (after_at, after_id) = cursor_binds(after);
    let rows = sqlx::query(
        "SELECT resolution.dispatch_id, resolution.contract_id, resolution.evidence_id,
                consumption.obligation_id, consumption.original_round, obligation.obligation_kind,
                resolution.resolved_at AS scan_at, resolution.dispatch_id AS scan_id
         FROM public.consignment_order_resolutions AS resolution
         JOIN public.consignment_return_consumptions AS consumption
           ON consumption.intake_id = resolution.evidence_id
          AND consumption.contract_id = resolution.contract_id
          AND consumption.leg = 'original'
         JOIN public.settlement_obligations AS obligation
           ON obligation.id = consumption.obligation_id
          AND obligation.contract_id = consumption.contract_id
         WHERE resolution.action = 'redispatch_original' AND resolution.dispatch_id IS NOT NULL
           AND ($1::uuid IS NULL OR resolution.dispatch_id = $1)
           AND NOT EXISTS (SELECT 1 FROM public.consignment_dispatch_rounds AS opened
                           WHERE opened.round_id = resolution.dispatch_id)
           AND ($3::timestamptz IS NULL
                OR (resolution.resolved_at, resolution.dispatch_id) > ($3::timestamptz, $4::uuid))
         ORDER BY resolution.resolved_at, resolution.dispatch_id
         LIMIT $2",
    )
    .bind(dispatch_id)
    .bind(limit)
    .bind(after_at)
    .bind(after_id)
    .fetch_all(connection)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(QueuedRow {
                scan: row_cursor(row)?,
                dispatch_id: row.try_get("dispatch_id")?,
                contract_id: row.try_get("contract_id")?,
                intake_id: row.try_get("evidence_id")?,
                obligation_id: row.try_get("obligation_id")?,
                obligation_kind: row.try_get("obligation_kind")?,
                original_round: row.try_get("original_round")?,
            })
        })
        .collect()
}

/// 新轮键与冻结身份基础；来源不符（非原件 COD 腿、轮 ID 不符、上一轮之后已有更新的轮、
/// 上一轮或合同缺失）返回 `None`。
async fn basis_of(
    connection: &mut PgConnection,
    row: &QueuedRow,
) -> Result<Option<Basis>, WriteError> {
    let original = u32::try_from(row.original_round)?;
    let key = DispatchRoundKey {
        contract_id: row.contract_id,
        obligation_id: row.obligation_id,
        round: original + 1,
    };
    if row.obligation_kind != UNPAID_ORIGINAL_KIND || dispatch_round_id(&key) != row.dispatch_id {
        return Ok(None);
    }
    if latest_round(&mut *connection, row.contract_id, row.obligation_id).await? != Some(original) {
        return Ok(None);
    }
    let prior_key = DispatchRoundKey { round: original, ..key };
    let Some(prior) = load_by_key(&mut *connection, &prior_key).await? else {
        return Ok(None);
    };
    let Some(world) = parse_settlement_world(&prior.world_id) else {
        return Ok(None);
    };
    // 受益人取合同冻结买方（原件 COD 腿行使买方交割权）；合同与 sale 只追加，无锁读取。
    let contract = sqlx::query(
        "SELECT sale.frozen_buyer_identity, sale.price, sale.payment_deadline_days,
                contract.canonical_eid,
                (SELECT snapshot.name FROM public.consignment_snapshots AS snapshot
                 WHERE snapshot.listing_id = contract.listing_id
                 ORDER BY snapshot.generation DESC LIMIT 1) AS item_name
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE contract.contract_id = $1",
    )
    .bind(row.contract_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(contract) = contract else {
        return Ok(None);
    };
    let Some(item_name) = contract.try_get::<Option<String>, _>("item_name")? else {
        return Ok(None);
    };
    Ok(Some(Basis {
        key,
        rights_holder_identity: contract.try_get("frozen_buyer_identity")?,
        bot_uid: prior.bot_uid,
        world,
        subject: prior.subject,
        template: prior.template,
        round_ref: dispatch_ref(&key),
        credits: prior.credits,
        cod: prior.cod,
        canonical_eid: contract.try_get("canonical_eid")?,
        prior_d0: prior.d0,
        item_name,
        price: contract.try_get("price")?,
        payment_deadline_days: contract.try_get("payment_deadline_days")?,
    }))
}

/// 新轮已备好的投递与其冻结身份；尚未备好返回 `None`。载荷形状不对或摘要不符同样返回 `None`。
async fn prepared_of(
    connection: &mut PgConnection,
    dispatch_id: Uuid,
) -> Result<Option<PreparedRedispatch>, WriteError> {
    let (delivery_id, _command_id) = round_delivery_ids(dispatch_id);
    let payload: Option<Value> = sqlx::query_scalar(
        "SELECT payload FROM public.deliveries WHERE id = $1 AND status = 'prepared'",
    )
    .bind(delivery_id)
    .fetch_optional(connection)
    .await?;
    Ok(payload
        .and_then(|payload| frozen_from_payload(&payload))
        .map(|frozen| PreparedRedispatch { delivery_id, frozen }))
}

/// 从补寄投递载荷还原冻结身份，并核对载荷的身份摘要（下游首轮的备好投递同形，共用）。
pub(super) fn frozen_from_payload(payload: &Value) -> Option<FrozenDispatchIdentity> {
    let text = |value: &Value, name: &str| value.get(name)?.as_str().map(str::to_owned);
    let extra = payload.get("frozen")?;
    let frozen = FrozenDispatchIdentity {
        rights_holder_identity: text(extra, "rights_holder_identity")?,
        bot_uid: text(extra, "bot_uid")?,
        world: parse_settlement_world(payload.get("world")?.as_str()?)?,
        subject: text(payload, "subject")?,
        template: text(payload, "template")?,
        body_digest: unhex(extra.get("body_digest")?.as_str()?)?,
        round_ref: text(payload, "round_ref")?,
        d0: serde_json::from_value::<Timestamp>(extra.get("d0")?.clone()).ok()?,
        credits: payload.get("credits")?.as_i64()?,
        cod: payload.get("cod")?.as_i64()?,
        equipment_ids: payload
            .get("equipment_ids")?
            .as_array()?
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()?,
    };
    payload_matches(payload, &frozen).then_some(frozen)
}

/// 32 字节摘要的小写十六进制解码；形状不对返回 `None`。
fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

/// 渲染内容来源要求的义务（补寄的付款义务、下游首轮的交割 / 卖款 / 退款义务共用）；列与
/// `contracts::source` 的义务映射同源，名称或数值损坏失败关闭。
pub(super) async fn load_payment(
    connection: &mut PgConnection,
    obligation_id: Uuid,
) -> Result<ConsignmentObligation, WriteError> {
    let row = sqlx::query(
        "SELECT o.id, o.world_id, o.obligation_kind, o.business_state, o.state_version,
                o.policy_version, o.recipient_user_id, o.recipient_game_identity, o.amount,
                o.carrier_credit, o.prerequisite_obligation_id, o.contract_id, o.contract_kind,
                o.custody_equipment_id
         FROM public.settlement_obligations AS o WHERE o.id = $1",
    )
    .bind(obligation_id)
    .fetch_one(connection)
    .await?;
    let corrupt = || WriteError::invariant("consignment_redispatch_obligation");
    let name = |column: &str| row.try_get::<String, _>(column);
    let world = SettlementWorld::from_name(&name("world_id")?).ok_or_else(corrupt)?;
    let kind =
        SettlementObligationKind::from_name(&name("obligation_kind")?).ok_or_else(corrupt)?;
    let state =
        SettlementObligationState::from_name(&name("business_state")?).ok_or_else(corrupt)?;
    let contract_kind = row
        .try_get::<Option<String>, _>("contract_kind")?
        .as_deref()
        .and_then(ConsignmentContractKind::parse)
        .ok_or_else(corrupt)?;
    let amount = SettlementAmount::new(row.try_get("amount")?).map_err(|error| {
        tracing::warn!(%error, "补寄付款义务金额无效");
        corrupt()
    })?;
    Ok(ConsignmentObligation {
        id: SettlementId::from_uuid(row.try_get("id")?),
        contract_id: ConsignmentContractId::from_uuid(
            row.try_get::<Option<Uuid>, _>("contract_id")?.ok_or_else(corrupt)?,
        ),
        contract_kind,
        world,
        kind,
        state,
        state_version: u64::try_from(row.try_get::<i64, _>("state_version")?)?,
        policy_version: u32::try_from(row.try_get::<i32, _>("policy_version")?)?,
        recipient_user_id: row
            .try_get::<Option<Uuid>, _>("recipient_user_id")?
            .map(UserId::from_uuid),
        recipient_game_identity: row
            .try_get::<Option<String>, _>("recipient_game_identity")?
            .unwrap_or_default(),
        amount,
        carrier_credit: u32::try_from(row.try_get::<i64, _>("carrier_credit")?)?,
        custody_equipment_id: row
            .try_get::<Option<Uuid>, _>("custody_equipment_id")?
            .ok_or_else(corrupt)?,
        prerequisite_id: row
            .try_get::<Option<Uuid>, _>("prerequisite_obligation_id")?
            .map(SettlementId::from_uuid),
    })
}

/// 渲染出的新轮冻结身份与存储确定的基础一致，且 D0 不早于上一轮。
fn conforms(frozen: &FrozenDispatchIdentity, basis: &Basis) -> bool {
    frozen.validate().is_ok()
        && frozen.subject == basis.subject
        && frozen.template == basis.template
        && frozen.equipment_ids.as_slice() == [basis.canonical_eid.clone()]
        && frozen.d0 >= basis.prior_d0
}

async fn prepare_in_tx(
    input: &(Uuid, Arc<dyn FirstDispatchContentSource>),
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Option<RedispatchPrepared>, WriteError> {
    let (dispatch_id, content) = input;
    let Some(row) =
        queued_rows(tx.connection(), Some(*dispatch_id), None, 1).await?.into_iter().next()
    else {
        return Ok(None);
    };
    let Some(basis) = basis_of(tx.connection(), &row).await? else {
        return Ok(None);
    };
    let (delivery_id, command_id) = round_delivery_ids(*dispatch_id);
    let existing: Option<Value> =
        sqlx::query_scalar("SELECT payload FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_optional(tx.connection())
            .await?;
    if let Some(payload) = existing {
        // 同一新轮至多一封投递：已备好即重放原投递与原冻结身份，不重渲染、不覆盖（不变量 6）。
        return Ok(frozen_from_payload(&payload).map(|frozen| RedispatchPrepared {
            delivery_id,
            frozen,
            replayed: true,
        }));
    }
    // 新轮 D0 = 本事务的数据库时刻 + 合同冻结的付款期限（维护者裁定）。
    let d0: Timestamp =
        sqlx::query_scalar("SELECT transaction_timestamp() + make_interval(days => $1::int)")
            .bind(i32::from(basis.payment_deadline_days))
            .fetch_one(tx.connection())
            .await?;
    let payment = load_payment(tx.connection(), row.obligation_id).await?;
    let price = ListingPrice::new(basis.price).map_err(|error| {
        tracing::warn!(%error, "补寄合同价格无效");
        WriteError::invariant("consignment_redispatch_price")
    })?;
    let request = FirstDispatchRequest {
        world: basis.world,
        price,
        round_ref: &basis.round_ref,
        payment_deadline_at: d0,
        item_name: &basis.item_name,
        canonical_eid: &basis.canonical_eid,
        payment: &payment,
    };
    // 内容不可得（渲染失败）整笔回滚，调用方稍后重试；与购买首轮同一来源。
    let rendered = content.content(&request).map_err(WriteError::from)?;
    let frozen = FrozenDispatchIdentity {
        rights_holder_identity: basis.rights_holder_identity.clone(),
        bot_uid: basis.bot_uid.clone(),
        world: basis.world,
        subject: rendered.subject,
        template: rendered.template,
        body_digest: rendered.body_digest,
        round_ref: basis.round_ref.clone(),
        d0,
        credits: basis.credits,
        cod: basis.cod,
        equipment_ids: rendered.equipment_ids,
    };
    if !conforms(&frozen, &basis) {
        return Ok(None);
    }
    let mut payload = send_payload(&frozen);
    // 授权时须原样给出冻结身份：轮行建立前，投递载荷是新轮冻结身份的唯一持久来源。
    payload["frozen"] = json!({
        "rights_holder_identity": frozen.rights_holder_identity,
        "bot_uid": frozen.bot_uid,
        "body_digest": hex(&frozen.body_digest),
        "d0": frozen.d0,
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, $3, 'settlement', $4, $5, $6, 'prepared')",
    )
    .bind(delivery_id)
    .bind(command_id)
    .bind(frozen.identity_digest().as_slice())
    .bind(basis.key.obligation_id.to_string())
    .bind(&basis.rights_holder_identity)
    .bind(&payload)
    .execute(tx.connection())
    .await?;
    let occurred_at = database_now(tx.connection()).await?;
    append_delivery_audit(
        tx.tracked(),
        DeliveryAudit {
            occurred_at,
            actor_type: "system",
            actor_id: "consignment-dispatch",
            action: PREPARE,
            delivery_id,
            request_id: None,
            source: "system",
            metadata: json!({ "origin_kind": "settlement", "status": "prepared" }),
        },
    )
    .await?;
    Ok(Some(RedispatchPrepared { delivery_id, frozen, replayed: false }))
}

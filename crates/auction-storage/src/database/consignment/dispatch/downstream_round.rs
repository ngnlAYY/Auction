//! 下游义务首轮的生产者（PIWP）：isekai 原件交割、卖款 S、退款 P 三种义务在放行后没有购买事务可随之建轮，
//! 由本模块为其备好首轮 `prepared` 投递，建轮仍只经 `authorize_dispatch`（首轮来源），起写与 POST 仍只经派发
//! executor 与一次性许可。另给出起写时复核“义务已就绪”的同一谓词。
//!
//! 保护不变量 3、4、5、6：
//! - 生产者的就绪谓词：义务 `ready`、前置履约已满足（`obligation_ready!`），且 funded 投影处于与该义务相符的
//!   处置（`funded_matches!`）——原件交割要求仍为 `funded`（未转退款），卖款要求 `original_claimed`（原件已确领），
//!   退款要求 `refund_converted`（P 已转入退款用途）。三者两两互斥，卖款与退款不会同时就绪（D9 §4.3 禁双派）。
//! - 起写复核（[`downstream_ready`]）要求义务已放行；原件交割腿另要求 funded 仍为 `funded`（已转退款就不能再寄
//!   原件，交割腿没有资金占用替它把关）。卖款与退款腿的金额互斥由起写同事务的资金占用把关：转退款后卖款用途为零、
//!   退款用途只在转换后才有 P。
//! - 首轮冻结身份全部在备好事务里由合同事实确定：权利人取义务受益人（卖款为合同冻结卖家，交割与退款为
//!   合同冻结买方），现金腿的 Bot UID 取成交冻结的资金 UID（发信 UID 必须等于它），交割腿取代管原件的 Bot；
//!   世界取履约行的派发世界（卖款与退款在 main）；Credits 取义务金额（卖款 S、退款 P，交割 0），COD 恒 0；
//!   D0 沿用购买首轮冻结的付款期限（同一合同的正文 D0 不变）；轮引用为首轮 ID。正文经与购买同源的冻结模板
//!   渲染，附件形状不符（交割必须恰为规范装备、现金腿不带装备）即失败关闭、不建投递。
//! - 投递与投递命令 ID 由首轮 ID 确定性派生，同一义务至多一封备好的投递；已备好时重放原投递与原冻结身份。
//!   本模块不外写、不改义务与资金。

use std::sync::Arc;

use auction_application::consignment::{ConsignmentError, ScanCursor, ScanPage};
use auction_domain::SettlementObligationKind;
use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref, dispatch_round_id,
};
use auction_domain::consignment::{ConsignmentObligation, parse_settlement_world};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::first_dispatch::{hex, round_delivery_ids, send_payload};
use super::redispatch::{frozen_from_payload, load_payment};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::purchase::FirstDispatchContent;
use crate::database::consignment::scan::{cursor_binds, next_cursor};
use crate::database::delivery::audit::{DeliveryAudit, PREPARE, append as append_delivery_audit};

/// 义务本身已放行（`o` 为义务行）：`ready` 且前置履约已满足。
macro_rules! obligation_ready {
    () => {
        "o.business_state = 'ready'
         AND EXISTS (
             SELECT 1 FROM public.settlement_obligation_fulfillments AS prerequisite
             WHERE prerequisite.obligation_id = o.prerequisite_obligation_id
               AND prerequisite.execution_state = 'satisfied')"
    };
}

/// funded 投影处于与该义务相符的处置（`o` 为义务行）。
macro_rules! funded_matches {
    () => {
        "EXISTS (
             SELECT 1 FROM public.consignment_funded_sales AS funded
             WHERE funded.contract_id = o.contract_id
               AND funded.disposition = CASE o.obligation_kind
                   WHEN 'winner_equipment_handoff' THEN 'funded'
                   WHEN 'seller_payout' THEN 'original_claimed'
                   WHEN 'buyer_refund' THEN 'refund_converted' END)"
    };
}

/// 下游义务种类闭集。
macro_rules! downstream_kinds {
    () => {
        "('winner_equipment_handoff', 'seller_payout', 'buyer_refund')"
    };
}

/// 冻结下游首轮内容所需的合同事实；不含装备 key、URL 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct DownstreamDispatchRequest<'a> {
    /// 派发世界（交割腿为原件所在世界，卖款与退款为 main）。
    pub world: SettlementWorld,
    /// 轮引用，即首轮 ID 的小写带连字符 UUID。
    pub round_ref: &'a str,
    /// 装备显示名，来自登记时的安全快照。
    pub item_name: &'a str,
    /// 规范装备编号。
    pub canonical_eid: &'a str,
    /// 本轮义务（原件交割、卖款或退款）。
    pub obligation: &'a ConsignmentObligation,
}

/// 由装配根实现：渲染冻结模板得出下游首轮内容。同步、无 I/O；不可得时返回 `Unavailable`，不建投递。
pub trait DownstreamDispatchContentSource: Send + Sync {
    /// 交割模板 `consignment_equipment_handoff`、卖款 `consignment_seller_payout`、退款
    /// `consignment_buyer_refund`。
    fn downstream_content(
        &self,
        request: &DownstreamDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError>;
}

/// 已备好首轮投递、待授权建轮的下游义务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDownstreamRound {
    /// 首轮业务键（合同、义务、轮 1）。
    pub key: DispatchRoundKey,
    /// 备好的投递。
    pub delivery_id: Uuid,
    /// 首轮冻结身份；授权时原样给出。
    pub frozen: FrozenDispatchIdentity,
}

/// 下游首轮生产者的存储来源；建轮由调用方经 `DispatchService::authorize` 完成。
#[derive(Clone)]
pub struct DownstreamRoundRepository {
    store: DatabaseStore,
    content: Arc<dyn DownstreamDispatchContentSource>,
}

impl DownstreamRoundRepository {
    /// 注入数据库与冻结模板内容来源。
    pub fn new(store: DatabaseStore, content: Arc<dyn DownstreamDispatchContentSource>) -> Self {
        Self { store, content }
    }

    /// 为游标之后至多 `limit` 项已就绪、尚未建轮的下游义务备好首轮投递（已备好的原样重放），按
    /// `(updated_at, 义务 ID)` 升序返回一页。单项来源不符或内容不可得只记安全码并跳过（不写任何行，下一轮仍会
    /// 被选中），但同样推进游标，异常项不会饿死后面的义务；读满一页才给出下一页游标。读取待办失败返回错误。
    pub async fn prepare_pending(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> Result<ScanPage<PreparedDownstreamRound>, ConsignmentError> {
        let (pending, next) =
            self.pending(after, limit).await.map_err(WriteError::into_application)?;
        let mut prepared = Vec::with_capacity(pending.len());
        for (contract_id, obligation_id) in pending {
            match self.prepare(contract_id, obligation_id).await {
                Ok(Some(round)) => prepared.push(round),
                Ok(None) => {
                    tracing::warn!(
                        code = "downstream_round_source_mismatch",
                        "下游首轮来源不符，不备投递"
                    );
                }
                Err(error) => {
                    let error = error.into_application();
                    tracing::warn!(%error, "下游首轮备好失败，稍后重试");
                }
            }
        }
        Ok(ScanPage { items: prepared, next })
    }

    /// 一页待办的 `(合同, 义务)` 与下一页游标（读满一页才有）。
    async fn pending(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> Result<(Vec<(Uuid, Uuid)>, Option<ScanCursor>), WriteError> {
        let mut transaction = self.store.pool().begin_with("BEGIN READ ONLY").await?;
        let rows = pending_rows(&mut transaction, None, after, i64::from(limit)).await?;
        transaction.commit().await?;
        let ids = rows
            .iter()
            .map(|row| Ok((row.try_get("contract_id")?, row.try_get("obligation_id")?)))
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok((ids, next_cursor(&rows, limit)?))
    }

    /// 命令锁键取首轮 ID（与补寄备好同口径）：同一义务的并发备好者在会话锁上排队；一个顶层 Serializable 事务。
    async fn prepare(
        &self,
        contract_id: Uuid,
        obligation_id: Uuid,
    ) -> Result<Option<PreparedDownstreamRound>, WriteError> {
        let round_id =
            dispatch_round_id(&DispatchRoundKey { contract_id, obligation_id, round: 1 });
        let keys = LockKeys { command_id: round_id, sellers: &[], equipment: &[] };
        let input = (obligation_id, Arc::clone(&self.content));
        with_deposit_transaction(&self.store, keys, &[], &input, |input, tx| {
            Box::pin(prepare_in_tx(input, tx))
        })
        .await
    }
}

/// 起写复核：下游义务（交割、卖款、退款）必须已放行，交割腿另须未转退款；其余义务不在范围内，返回 `true`。
pub(super) async fn downstream_ready(
    connection: &mut PgConnection,
    obligation_id: Uuid,
) -> Result<bool, WriteError> {
    let ready: Option<bool> = sqlx::query_scalar(concat!(
        "SELECT CASE WHEN o.obligation_kind IN ",
        downstream_kinds!(),
        " THEN (",
        obligation_ready!(),
        " AND (o.obligation_kind <> 'winner_equipment_handoff' OR ",
        funded_matches!(),
        ")) ELSE true END
         FROM public.settlement_obligations AS o WHERE o.id = $1"
    ))
    .bind(obligation_id)
    .fetch_optional(connection)
    .await?;
    Ok(ready == Some(true))
}

/// 已就绪、尚未建轮、履约行仍是初始 `pending` 且未衔接投递的下游义务。
async fn pending_rows(
    connection: &mut PgConnection,
    obligation_id: Option<Uuid>,
    after: Option<ScanCursor>,
    limit: i64,
) -> Result<Vec<PgRow>, WriteError> {
    let (after_at, after_id) = cursor_binds(after);
    let rows = sqlx::query(concat!(
        "SELECT o.contract_id, o.id AS obligation_id, o.updated_at AS scan_at, o.id AS scan_id
         FROM public.settlement_obligations AS o
         JOIN public.settlement_obligation_fulfillments AS f ON f.obligation_id = o.id
         WHERE o.contract_id IS NOT NULL AND o.obligation_kind IN ",
        downstream_kinds!(),
        " AND ($1::uuid IS NULL OR o.id = $1)
           AND f.execution_state = 'pending' AND f.delivery_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM public.consignment_dispatch_rounds AS round
                           WHERE round.contract_id = o.contract_id AND round.obligation_id = o.id)
           AND ",
        obligation_ready!(),
        " AND ",
        funded_matches!(),
        " AND ($3::timestamptz IS NULL OR (o.updated_at, o.id) > ($3::timestamptz, $4::uuid))
           ORDER BY o.updated_at, o.id LIMIT $2"
    ))
    .bind(obligation_id)
    .bind(limit)
    .bind(after_at)
    .bind(after_id)
    .fetch_all(connection)
    .await?;
    Ok(rows)
}

/// 首轮冻结身份中由合同事实确定的部分。
struct Basis {
    key: DispatchRoundKey,
    rights_holder_identity: String,
    bot_uid: String,
    world: SettlementWorld,
    canonical_eid: String,
    item_name: String,
    d0: Timestamp,
}

async fn basis_of(
    connection: &mut PgConnection,
    obligation: &ConsignmentObligation,
) -> Result<Option<Basis>, WriteError> {
    let key = DispatchRoundKey {
        contract_id: obligation.contract_id.as_uuid(),
        obligation_id: obligation.id.as_uuid(),
        round: 1,
    };
    // 合同、sale 与首轮行只追加，无锁读取；交割腿的 Bot 取代管原件的整封所在 Bot（与撤回件首轮同源）。
    let row = sqlx::query(
        "SELECT contract.canonical_eid, contract.frozen_seller_identity, sale.frozen_buyer_identity,
                sale.funding_uid, fulfillment.dispatch_world, item.bot_uid AS custody_bot_uid,
                (SELECT snapshot.name FROM public.consignment_snapshots AS snapshot
                 WHERE snapshot.listing_id = contract.listing_id
                 ORDER BY snapshot.generation DESC LIMIT 1) AS item_name,
                (SELECT first.d0 FROM public.consignment_dispatch_rounds AS first
                 JOIN public.settlement_obligations AS payment ON payment.id = first.obligation_id
                 WHERE first.contract_id = contract.contract_id AND first.round = 1
                   AND payment.obligation_kind IN
                       ('winner_equipment_cod', 'winner_payment_carrier_cod')) AS d0
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = $2
         JOIN public.consignment_custody_positions AS position ON position.id = contract.position_id
         JOIN public.consignment_deposit_items AS item ON item.id = position.item_id
         WHERE contract.contract_id = $1",
    )
    .bind(key.contract_id)
    .bind(key.obligation_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (Some(item_name), Some(d0)) = (
        row.try_get::<Option<String>, _>("item_name")?,
        row.try_get::<Option<Timestamp>, _>("d0")?,
    ) else {
        return Ok(None);
    };
    let Some(world) = parse_settlement_world(&row.try_get::<String, _>("dispatch_world")?) else {
        return Ok(None);
    };
    let (rights_holder_identity, bot_uid) = match obligation.kind {
        SettlementObligationKind::SellerPayout => {
            (row.try_get("frozen_seller_identity")?, row.try_get("funding_uid")?)
        }
        SettlementObligationKind::BuyerRefund => {
            (row.try_get("frozen_buyer_identity")?, row.try_get("funding_uid")?)
        }
        SettlementObligationKind::WinnerEquipmentHandoff => {
            (row.try_get("frozen_buyer_identity")?, row.try_get("custody_bot_uid")?)
        }
        _ => return Ok(None),
    };
    Ok(Some(Basis {
        key,
        rights_holder_identity,
        bot_uid,
        world,
        canonical_eid: row.try_get("canonical_eid")?,
        item_name,
        d0,
    }))
}

/// 冻结 Credits 与附件形状：现金腿 Credits 为义务金额、不带装备，交割腿零 Credits、恰为规范装备。
fn shape_of(obligation: &ConsignmentObligation, canonical_eid: &str) -> (i64, Vec<String>) {
    match obligation.kind {
        SettlementObligationKind::WinnerEquipmentHandoff => (0, vec![canonical_eid.to_owned()]),
        _ => (obligation.amount.value(), Vec::new()),
    }
}

async fn prepare_in_tx(
    input: &(Uuid, Arc<dyn DownstreamDispatchContentSource>),
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Option<PreparedDownstreamRound>, WriteError> {
    let (obligation_id, content) = input;
    // 锁内重核待办：已建轮、已衔接或不再就绪时不备。
    if pending_rows(tx.connection(), Some(*obligation_id), None, 1).await?.is_empty() {
        return Ok(None);
    }
    let obligation = load_payment(tx.connection(), *obligation_id).await?;
    let Some(basis) = basis_of(tx.connection(), &obligation).await? else {
        return Ok(None);
    };
    let round_id = dispatch_round_id(&basis.key);
    let (delivery_id, command_id) = round_delivery_ids(round_id);
    let existing: Option<Value> =
        sqlx::query_scalar("SELECT payload FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_optional(tx.connection())
            .await?;
    if let Some(payload) = existing {
        // 同一首轮至多一封投递：已备好即重放原投递与原冻结身份，不重渲染、不覆盖（不变量 6）。
        return Ok(frozen_from_payload(&payload).map(|frozen| PreparedDownstreamRound {
            key: basis.key,
            delivery_id,
            frozen,
        }));
    }
    let round_ref = dispatch_ref(&basis.key);
    // 内容不可得（渲染失败）整笔回滚，下一轮重试；与购买首轮同一冻结模板来源。
    let rendered = content.downstream_content(&DownstreamDispatchRequest {
        world: basis.world,
        round_ref: &round_ref,
        item_name: &basis.item_name,
        canonical_eid: &basis.canonical_eid,
        obligation: &obligation,
    })?;
    let (credits, equipment_ids) = shape_of(&obligation, &basis.canonical_eid);
    let frozen = FrozenDispatchIdentity {
        rights_holder_identity: basis.rights_holder_identity.clone(),
        bot_uid: basis.bot_uid.clone(),
        world: basis.world,
        subject: rendered.subject,
        template: rendered.template,
        body_digest: rendered.body_digest,
        round_ref,
        d0: basis.d0,
        credits,
        cod: 0,
        equipment_ids: rendered.equipment_ids,
    };
    // 不变量 6：附件必须就是该腿应有的形状（交割寄规范原件，现金腿只带 Credits），否则不发错件。
    if frozen.validate().is_err() || frozen.equipment_ids != equipment_ids {
        return Ok(None);
    }
    insert_prepared(tx, &basis, &frozen, delivery_id, command_id).await?;
    Ok(Some(PreparedDownstreamRound { key: basis.key, delivery_id, frozen }))
}

/// 写 `prepared` 投递（收件人即冻结权利人）与准备审计；载荷另带授权时原样给出的冻结段（与补寄同形）。
async fn insert_prepared(
    tx: &mut DepositTransaction<'_, '_>,
    basis: &Basis,
    frozen: &FrozenDispatchIdentity,
    delivery_id: Uuid,
    command_id: Uuid,
) -> Result<(), WriteError> {
    let mut payload = send_payload(frozen);
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
    Ok(())
}

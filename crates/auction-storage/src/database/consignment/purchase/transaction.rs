//! 购买事务：命令仲裁 → 锁内事实 → 判定 → 一次写 Sale、义务图、首轮、position、挂单、事件与回执。
//!
//! 保护不变量 1、2、3、6、10：同命令 ID 同请求回原回执，异请求冲突；事实在命令、买卖双方用户与
//! 规范实物会话锁之后的 Serializable 事务内读出；Sale、义务图、首轮、`committed`、`sale_pending`、
//! 事件与回执同事务提交，任一步失败整笔回滚；成交金额与 D0 只由冻结价格、费率与策略推出。

use auction_application::consignment::{
    ConsignmentError, PurchaseCommand, PurchaseOutcome, PurchaseReceipt,
};
use auction_application::settlement::{FundingError, FundingPreflightFact};
use auction_domain::consignment::dispatch::{DispatchRoundKey, dispatch_ref};
use auction_domain::consignment::purchase::{
    PurchaseFunding, PurchaseGrant, PurchaseRejection, PurchaseSaleInput,
    build_purchase_sale_terms, decide_purchase,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ListingEquipmentIdentity, OrderState,
    build_consignment_obligations, settlement_world_code,
};
use auction_domain::{CommandId, ConsignmentContractId, UserId, legacy_natural_key_uuid};
use auction_time::Timestamp;
use serde_json::{Value, json};
use uuid::Uuid;

use super::PurchaseRepository;
use super::content::{FirstDispatchRequest, frozen_identity};
use super::facts::{PurchaseLoad, load_purchase, lock_listing_row};
use super::ledger::{
    Arbitration, CommandIdentity, Operation, StoredReceipt, arbitrate, insert_receipt,
    rejection_receipt,
};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::contracts::graph::write_graph_in_conn;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::dispatch::create_first_round_in_tx;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::consignment::rights::register_contract_rights_in_tx;

/// 合同 ID 的派生 namespace；合同与来源命令一一对应，重放同一购买必得同一合同。字节冻结。
const CONTRACT_ID_NAMESPACE: &str = "consignment.sale-contract.v1";

/// 购买事务的输入：仓库、命令与预读的挂单卖家。
pub(super) struct PurchaseCall<'a> {
    pub(super) repository: &'a PurchaseRepository,
    pub(super) command: &'a PurchaseCommand,
    /// 锁键预读的卖家；挂单不存在时为空。预读只准备锁键，不具授权含义。
    pub(super) pre_read_seller: Option<Uuid>,
    /// 开事务前读出的资金预检；挂单不存在或世界无法解析时为空。
    pub(super) funding: Option<Result<FundingPreflightFact, FundingError>>,
}

/// 一个购买事务（一次重试尝试）。
pub(super) async fn purchase_in_tx(
    call: &PurchaseCall<'_>,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<PurchaseOutcome, WriteError> {
    let command = call.command;
    let listing_id = command.input.listing_id;
    let identity = CommandIdentity {
        command_id: command.command_id.as_uuid(),
        user_id: command.buyer_user_id.as_uuid(),
        operation: Operation::Purchase,
        listing_id,
        digest: command.request_hash,
        request_id: command.request_id.as_deref(),
    };
    match arbitrate(tx.connection(), &identity).await? {
        Arbitration::Replay(stored) => return decode_replay(&stored, command.command_id),
        Arbitration::First => {}
    }
    let now = database_now(tx.connection()).await?;
    let Some(listing) = lock_listing_row(tx.connection(), listing_id).await? else {
        // 挂单不存在：没有可挂靠回执的目标，确定拒绝不入账；预读后才出现则要求原样重试。
        return match call.pre_read_seller {
            None => Ok(PurchaseOutcome::Rejected(PurchaseRejection::ListingNotActive)),
            Some(_) => Err(ConsignmentError::Unavailable.into()),
        };
    };
    if call.pre_read_seller != Some(listing.seller_user_id) {
        // 锁集合与锁内事实不符：退出重规划，不在快照内补锁。
        return Err(ConsignmentError::Unavailable.into());
    }
    let Some(funding) = &call.funding else {
        return Err(ConsignmentError::Unavailable.into());
    };
    let load =
        load_purchase(tx.connection(), funding, &listing, command.buyer_user_id.as_uuid(), now)
            .await?;
    match decide_purchase(&command.input, &load.facts) {
        Err(rejection) => {
            let receipt = rejection_receipt(&identity, rejection.http_status(), rejection.code())?;
            insert_receipt(tx.connection(), &identity, &receipt, now).await?;
            Ok(PurchaseOutcome::Rejected(rejection))
        }
        Ok(grant) => write_purchase(call, tx, &identity, load, &grant, now).await,
    }
}

async fn write_purchase(
    call: &PurchaseCall<'_>,
    tx: &mut DepositTransaction<'_, '_>,
    identity: &CommandIdentity<'_>,
    load: PurchaseLoad,
    grant: &PurchaseGrant,
    now: Timestamp,
) -> Result<PurchaseOutcome, WriteError> {
    let command = call.command;
    let listing = load.facts.listing;
    let Some(context) = load.context else {
        return Err(WriteError::invariant("consignment_purchase_context"));
    };
    let terms = build_purchase_sale_terms(PurchaseSaleInput {
        committed_at: now,
        buyer_user_id: command.buyer_user_id,
        frozen_buyer_identity: context.buyer_identity.clone(),
        // 冻结的是成交所依据的版本；成交后的新版本记在回执与事件里。
        listing_version: command.input.expected_version,
        grant: grant.clone(),
        policy: call.repository.policy,
    })
    .map_err(contract_error)?;
    let contract_id = ConsignmentContractId::from_uuid(legacy_natural_key_uuid(
        CONTRACT_ID_NAMESPACE,
        &[command.command_id.as_uuid().as_bytes()],
    ));
    let contract = ConsignmentContract::Sale {
        core: ConsignmentContractCore {
            contract_id,
            origin_command_id: command.command_id,
            listing_id: listing.listing_id,
            position_id: context.position.position_id,
            generation: grant.generation,
            custody_equipment_id: context.position.equipment_id,
            world: listing.world,
            canonical_eid: context.position.canonical_eid.clone(),
            season_window_id: context.position.season_window_id,
            seller_user_id: load.facts.seller.user_id,
            frozen_seller_identity: context.seller_identity.clone(),
            created_at: now,
        },
        terms,
    };
    let graph = build_consignment_obligations(&contract).map_err(contract_error)?;

    // 写序：回执行 → 合同图 → 首轮 → position → 事件 → 挂单。回执先行，使后续行的同事务核对可见命令。
    let receipt = StoredReceipt {
        status: 201,
        code: "ok".to_owned(),
        body: json!({
            "schema_version": 2,
            "operation": Operation::Purchase.as_str(),
            "disposition": "purchased",
            "command_id": command.command_id.as_uuid().to_string(),
            "listing_id": listing.listing_id,
            "listing_version": grant.listing_version_after.value(),
            "state": "sale_pending",
            "contract_id": contract_id.as_uuid(),
            "contract_version": 1,
            "equipment_id": context.position.equipment_id,
            "world": settlement_world_code(listing.world),
            "price": grant.price.value(),
            "order_state": "awaiting_payment",
        }),
    };
    insert_receipt(tx.connection(), identity, &receipt, now).await?;
    write_graph_in_conn(tx.connection(), &graph).await?;
    // 不变量 1、3：买方与卖方权利和合同同事务登记；同合同同角色至多一行。
    register_contract_rights_in_tx(tx.connection(), contract_id.as_uuid(), now).await?;

    let payment = graph
        .obligations
        .first()
        .ok_or_else(|| WriteError::invariant("consignment_purchase_graph_empty"))?;
    let round_ref = dispatch_ref(&DispatchRoundKey {
        contract_id: contract_id.as_uuid(),
        obligation_id: payment.id.as_uuid(),
        round: 1,
    });
    let PurchaseFunding::Ready(funding) = &load.facts.funding else {
        return Err(WriteError::invariant("consignment_purchase_funding"));
    };
    let request = FirstDispatchRequest {
        world: listing.world,
        price: grant.price,
        round_ref: &round_ref,
        payment_deadline_at: payment_deadline(&contract)?,
        item_name: &context.item_name,
        canonical_eid: &context.position.canonical_eid,
        payment,
    };
    let content = call.repository.content.content(&request)?;
    // 首轮行使的是买方交割权：冻结的权利人即收件受益人（买方），与投递收件人、权利行同源。
    let frozen = frozen_identity(&request, content, &context.buyer_identity, &funding.uid)?;
    create_first_round_in_tx(tx.tracked(), &graph, &frozen).await?;

    commit_position(tx, &context.position, listing.listing_id, grant, command.command_id, now)
        .await?;
    stage_sale_pending(tx, &listing, grant, command.command_id, now).await?;
    // 不变量 3：订单阶段事件与权利刷新同成交事实同事务提交，回滚时一并消失。
    record_order_stage_change_in_tx(tx.connection(), contract_id.as_uuid(), now).await?;

    Ok(PurchaseOutcome::Purchased(PurchaseReceipt {
        command_id: command.command_id,
        listing_id: listing.listing_id,
        listing_version: grant.listing_version_after,
        contract_id,
        contract_version: 1,
        equipment_id: context.position.equipment_id,
        world: listing.world,
        price: grant.price,
        order_state: OrderState::AwaitingPayment,
        replayed: false,
    }))
}

fn payment_deadline(contract: &ConsignmentContract) -> Result<Timestamp, WriteError> {
    match contract {
        ConsignmentContract::Sale { terms, .. } => Ok(terms.payment_deadline_at),
        ConsignmentContract::Withdrawal { .. } => {
            Err(WriteError::invariant("consignment_purchase_contract_kind"))
        }
    }
}

/// position `in_custody → committed` 与同版本代管事件；000136 的触发器再核同事务存在该代的 Sale 合同。
async fn commit_position(
    tx: &mut DepositTransaction<'_, '_>,
    position: &super::facts::PositionRow,
    listing_id: Uuid,
    grant: &PurchaseGrant,
    command_id: CommandId,
    now: Timestamp,
) -> Result<(), WriteError> {
    let version: Option<i64> = sqlx::query_scalar(
        "UPDATE public.consignment_custody_positions
         SET state = 'committed', version = version + 1, updated_at = $3
         WHERE id = $1 AND state = 'in_custody' AND version = $2 RETURNING version",
    )
    .bind(position.position_id)
    .bind(position.version)
    .bind(now)
    .fetch_optional(tx.connection())
    .await?;
    let Some(version) = version else {
        return Err(WriteError::invariant("consignment_purchase_position_version"));
    };
    sqlx::query(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ($1, $2, $3, 'committed', 'in_custody', 'committed', NULL, $4, $5, $6)",
    )
    .bind(position.position_id)
    .bind(version)
    .bind(command_id.as_uuid())
    .bind(listing_id)
    .bind(i64::try_from(grant.listing_version_after.value())?)
    .bind(now)
    .execute(tx.connection())
    .await?;
    Ok(())
}

/// 挂单 `available → sale_pending` 与同版本 `purchased` 事件；价格、快照与期限不变。
async fn stage_sale_pending(
    tx: &mut DepositTransaction<'_, '_>,
    listing: &auction_domain::consignment::purchase::PurchaseListing,
    grant: &PurchaseGrant,
    command_id: CommandId,
    now: Timestamp,
) -> Result<(), WriteError> {
    let version = i64::try_from(grant.listing_version_after.value())?;
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ($1, $2, $3, 'public_user', 'purchased', 'available', 'sale_pending', NULL, $4, $5)",
    )
    .bind(listing.listing_id)
    .bind(version)
    .bind(command_id.as_uuid())
    .bind(grant.price.value())
    .bind(now)
    .execute(tx.connection())
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_listings
         SET state = 'sale_pending', state_version = $3, updated_at = $4
         WHERE id = $1 AND state_version = $2 AND state = 'available'",
    )
    .bind(listing.listing_id)
    .bind(i64::try_from(listing.version.value())?)
    .bind(version)
    .bind(now)
    .execute(tx.connection())
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_purchase_listing_version"));
    }
    Ok(())
}

/// 合同或义务图构造失败：失败关闭，只记固定原因。
fn contract_error(error: auction_domain::consignment::ConsignmentContractError) -> WriteError {
    tracing::warn!(%error, "寄售成交合同未通过校验");
    WriteError::invariant("consignment_purchase_contract")
}

/// 本人回执查询按命令 ID 读到的账本行，与购买重放走同一解码（不变量 1）。
pub(crate) fn decode_owned_purchase(
    status: i16,
    code: String,
    body: Value,
    command_id: CommandId,
) -> Result<PurchaseOutcome, WriteError> {
    decode_replay(&StoredReceipt { status, code, body }, command_id)
}

/// 把已存回执还原为端口结果；拒绝回执只恢复安全码。
pub(super) fn decode_replay(
    stored: &StoredReceipt,
    command_id: CommandId,
) -> Result<PurchaseOutcome, WriteError> {
    if stored.status >= 400 {
        let rejection = PurchaseRejection::ALL
            .into_iter()
            .find(|rejection| rejection.code() == stored.code)
            .ok_or_else(|| WriteError::invariant("consignment_purchase_rejection_code"))?;
        return Ok(PurchaseOutcome::Rejected(rejection));
    }
    let body = &stored.body;
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| WriteError::invariant("consignment_purchase_receipt_field"))
    };
    let uuid = |key: &str| {
        let Ok(value) = Uuid::parse_str(text(key)?) else {
            return Err(WriteError::invariant("consignment_purchase_receipt_field"));
        };
        Ok(value)
    };
    let number = |key: &str| {
        body.get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| WriteError::invariant("consignment_purchase_receipt_field"))
    };
    let world = auction_domain::consignment::parse_settlement_world(text("world")?)
        .ok_or_else(|| WriteError::invariant("consignment_purchase_receipt_field"))?;
    let price = body
        .get("price")
        .and_then(Value::as_i64)
        .ok_or_else(|| WriteError::invariant("consignment_purchase_receipt_field"))?;
    Ok(PurchaseOutcome::Purchased(PurchaseReceipt {
        command_id,
        listing_id: uuid("listing_id")?,
        listing_version: auction_domain::consignment::ListingVersion::new(number(
            "listing_version",
        )?)?,
        contract_id: ConsignmentContractId::from_uuid(uuid("contract_id")?),
        contract_version: number("contract_version")?,
        equipment_id: uuid("equipment_id")?,
        world,
        price: auction_domain::consignment::ListingPrice::new(price)?,
        order_state: OrderState::AwaitingPayment,
        replayed: true,
    }))
}

/// 买家与卖家的会话锁集合：买家永远在内，卖家与规范实物来自预读。
pub(super) fn lock_users(buyer: UserId, seller: Option<Uuid>) -> Vec<Uuid> {
    let mut users = vec![buyer.as_uuid()];
    users.extend(seller);
    users
}

/// 预读出的规范实物锁键。
pub(super) fn lock_equipment(
    identity: Option<&ListingEquipmentIdentity>,
) -> Vec<ListingEquipmentIdentity> {
    identity.cloned().into_iter().collect()
}

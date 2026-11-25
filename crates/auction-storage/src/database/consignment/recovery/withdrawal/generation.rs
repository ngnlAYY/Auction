//! 撤回涉及的 position 与处分代事实读取，以及 Withdrawal 合同图的绑定。
//!
//! 保护不变量 2、3、6：事实只在调用方已持有命令、卖家、实物与挂单锁之后读出；合同、义务与履约行
//! 经 `write_graph_in_conn` 同事务写入，`(position, generation)` 与来源命令的唯一约束兜底，
//! 并发撤回与购买至多一方取得该代；撤回件首轮（返还义务的派发轮与投递）随合同同事务建立（PIWX）。

use std::sync::Arc;

use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::CurrentGeneration;
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, WithdrawalTerms, build_consignment_obligations,
    parse_settlement_world,
};
use auction_domain::{CommandId, ConsignmentContractId, UserId, legacy_natural_key_uuid};
use auction_time::Timestamp;
use sqlx::{PgConnection, Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::contracts::graph::{
    WithdrawalChain, write_graph_in_conn, write_graph_with_chain,
};
use crate::database::consignment::deposit::primitives::SeasonWindow;
use crate::database::consignment::dispatch::create_withdrawal_round_in_tx;
use crate::database::consignment::purchase::FirstDispatchContentSource;
use crate::database::consignment::rights::register_contract_rights_in_tx;

/// Withdrawal 合同 ID 的派生 namespace；合同与来源命令一一对应，重放得到同一合同。字节冻结。
const WITHDRAWAL_CONTRACT_ID_NAMESPACE: &str = "consignment.withdrawal-contract.v1";

/// 一个挂单唯一的 position 在锁内的撤回相关事实。
pub(in crate::database::consignment::recovery) struct WithdrawalPosition {
    pub(in crate::database::consignment::recovery) position_id: Uuid,
    pub(in crate::database::consignment::recovery) seller_user_id: Uuid,
    pub(in crate::database::consignment::recovery) equipment_id: Uuid,
    pub(in crate::database::consignment::recovery) world_id: String,
    pub(in crate::database::consignment::recovery) canonical_eid: String,
    pub(in crate::database::consignment::recovery) season_window_id: Option<Uuid>,
    pub(in crate::database::consignment::recovery) state: PositionState,
    pub(in crate::database::consignment::recovery) version: i64,
    pub(in crate::database::consignment::recovery) generation: Option<u64>,
}

/// position 行的读取列；锁定读与快照读共用，只差末尾的 `FOR UPDATE`。
macro_rules! position_sql {
    () => {
        "SELECT id, seller_user_id, equipment_id, world_id, canonical_eid, season_window_id,
                state, version, current_generation
         FROM public.consignment_custody_positions WHERE listing_id = $1 ORDER BY id"
    };
}

/// 锁定挂单对应的 position；一单至多一个，多于一个是数据损坏，失败关闭。
pub(in crate::database::consignment::recovery) async fn lock_withdrawal_position(
    connection: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<WithdrawalPosition>, WriteError> {
    load_withdrawal_position(connection, listing_id, concat!(position_sql!(), " FOR UPDATE")).await
}

/// 不取锁读取挂单对应的 position，供只读快照判定“此刻可否发起”；写端仍须锁内重读。
pub(in crate::database::consignment::recovery) async fn read_withdrawal_position(
    connection: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<WithdrawalPosition>, WriteError> {
    load_withdrawal_position(connection, listing_id, position_sql!()).await
}

async fn load_withdrawal_position(
    connection: &mut PgConnection,
    listing_id: Uuid,
    sql: &'static str,
) -> Result<Option<WithdrawalPosition>, WriteError> {
    let rows = sqlx::query(sql).bind(listing_id).fetch_all(connection).await?;
    if rows.len() > 1 {
        return Err(WriteError::invariant("consignment_withdrawal_position_count"));
    }
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let state: &str = row.try_get("state")?;
    Ok(Some(WithdrawalPosition {
        position_id: row.try_get("id")?,
        seller_user_id: row.try_get("seller_user_id")?,
        equipment_id: row.try_get("equipment_id")?,
        world_id: row.try_get("world_id")?,
        canonical_eid: row.try_get("canonical_eid")?,
        season_window_id: row.try_get("season_window_id")?,
        state: PositionState::parse(state)
            .ok_or_else(|| WriteError::invariant("consignment_withdrawal_position_state"))?,
        version: row.try_get("version")?,
        generation: row
            .try_get::<Option<i64>, _>("current_generation")?
            .map(u64::try_from)
            .transpose()?,
    }))
}

/// 当前处分代相对买方权利的状态。
///
/// 不变量 2、4、6：
/// - 当前代没有合同，且其他代的合同都是已封闭的 Sale（重列后的新代）→ `Unsold`；
/// - 当前代是 Sale，已有封闭事实（未付取消或退款封闭）且原件从未离库或已精确收回 → `SaleSealed`，
///   证明引用取该 Sale 的合同 ID；
/// - 其余一律 `SaleOpen`：未封闭、原件可能在途、Withdrawal 已占代，或无法读清，都失败关闭。
pub(in crate::database::consignment::recovery) async fn current_generation(
    connection: &mut PgConnection,
    position: &WithdrawalPosition,
) -> Result<CurrentGeneration, WriteError> {
    let Some(generation) = position.generation else {
        return Ok(CurrentGeneration::SaleOpen);
    };
    let contracts: Vec<(i64, String, Uuid, bool, bool)> = sqlx::query_as(
        "SELECT contract.generation, contract.kind, contract.contract_id,
                closure.contract_id IS NOT NULL,
                public.c7_sale_original_back(contract.contract_id)
         FROM public.consignment_contracts AS contract
         LEFT JOIN public.consignment_sale_closures AS closure
           ON closure.contract_id = contract.contract_id
         WHERE contract.position_id = $1",
    )
    .bind(position.position_id)
    .fetch_all(&mut *connection)
    .await?;
    let current = i64::try_from(generation)?;
    let mut sealed_current = None;
    for (contract_generation, kind, contract_id, closed, original_back) in contracts {
        let sealed_sale = kind == "sale" && closed;
        if contract_generation == current {
            if !sealed_sale || !original_back {
                return Ok(CurrentGeneration::SaleOpen);
            }
            sealed_current = Some(contract_id);
        } else if !sealed_sale {
            // 其他代的 Withdrawal 或未封闭 Sale：位置已被占用或交割未清，失败关闭。
            return Ok(CurrentGeneration::SaleOpen);
        }
    }
    Ok(match sealed_current {
        Some(predecessor_proof) => CurrentGeneration::SaleSealed { generation, predecessor_proof },
        None => CurrentGeneration::Unsold,
    })
}

/// 该 position 的任一合同下是否存在结果未知的派发（不变量 4、6；D9 §5.3 “无相反未知 Send / 无未知派发”）。
///
/// 覆盖同合同下的全部腿（原件、载体、`buyer_refund` 的 cash_refund、`seller_payout` 的 cash_payout）：
/// 已起写（有 `delivery_send_attempts`）却没有 Sent MID 绑定，或投递已进入 `manual_reconcile`，都算未知。
/// 已绑定 Sent MID 的派发结果已证，不算未知。
pub(in crate::database::consignment::recovery) async fn unknown_dispatch_open(
    connection: &mut PgConnection,
    position_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM public.consignment_contracts AS contract
             JOIN public.consignment_dispatch_rounds AS round_row
               ON round_row.contract_id = contract.contract_id
             JOIN public.deliveries AS delivery ON delivery.id = round_row.delivery_id
             JOIN public.delivery_send_attempts AS send_attempt
               ON send_attempt.delivery_id = delivery.id
             WHERE contract.position_id = $1
               AND (delivery.status = 'manual_reconcile'
                    OR NOT EXISTS (
                        SELECT 1 FROM public.delivery_sent_bindings AS sent
                        WHERE sent.delivery_id = send_attempt.delivery_id
                          AND sent.fencing_version = send_attempt.fencing_version)))",
    )
    .bind(position_id)
    .fetch_one(connection)
    .await?)
}

/// 上代封闭证明引用：封闭种类与 Sale 合同 ID，只含固定词与 UUID，不含任何装备 key 或 URL。
pub(in crate::database::consignment::recovery) async fn closure_proof_ref(
    connection: &mut PgConnection,
    sale_contract_id: Uuid,
) -> Result<String, WriteError> {
    let kind: String = sqlx::query_scalar(
        "SELECT closure_kind FROM public.consignment_sale_closures WHERE contract_id = $1",
    )
    .bind(sale_contract_id)
    .fetch_one(connection)
    .await?;
    Ok(format!("sale-closure:{kind}:{sale_contract_id}"))
}

/// position 冻结的季窗；没有季窗的 isekai 件由调用方按季态未知失败关闭。
pub(in crate::database::consignment::recovery) async fn position_season_window(
    connection: &mut PgConnection,
    position: &WithdrawalPosition,
) -> Result<Option<SeasonWindow>, WriteError> {
    let Some(id) = position.season_window_id else { return Ok(None) };
    let row = sqlx::query(
        "SELECT start_utc, end_utc, admission_cutoff_utc
         FROM public.consignment_season_windows WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SeasonWindow {
        id,
        start_utc: row.try_get("start_utc")?,
        end_utc: row.try_get("end_utc")?,
        admission_cutoff_utc: row.try_get("admission_cutoff_utc")?,
    }))
}

/// 绑定一份 Withdrawal 所需的输入。
pub(in crate::database::consignment::recovery) struct NewWithdrawal<'a> {
    pub(in crate::database::consignment::recovery) origin_command_id: Uuid,
    pub(in crate::database::consignment::recovery) listing_id: Uuid,
    pub(in crate::database::consignment::recovery) position_id: Uuid,
    pub(in crate::database::consignment::recovery) generation: u64,
    pub(in crate::database::consignment::recovery) equipment_id: Uuid,
    pub(in crate::database::consignment::recovery) world_id: &'a str,
    pub(in crate::database::consignment::recovery) canonical_eid: &'a str,
    pub(in crate::database::consignment::recovery) season_window_id: Option<Uuid>,
    pub(in crate::database::consignment::recovery) seller_user_id: Uuid,
    pub(in crate::database::consignment::recovery) intent_id: Option<Uuid>,
    pub(in crate::database::consignment::recovery) now: Timestamp,
}

/// Sale 已封闭后撤回代的前驱：被取代的 Sale、前驱代序号与上代封闭证明引用。
pub(in crate::database::consignment::recovery) struct SealedPredecessor<'a> {
    pub(in crate::database::consignment::recovery) sale_contract_id: Uuid,
    pub(in crate::database::consignment::recovery) generation: u64,
    pub(in crate::database::consignment::recovery) closure_proof_ref: &'a str,
}

/// 在当前代上写 Withdrawal 合同、唯一的返还义务、初始履约行与撤回件首轮，返回合同 ID。
///
/// 不变量 3：返还义务自带的履约行加首轮就是唯一工作，不另建工作表；调用方在同一事务写挂单投影、
/// 事件与回执。
pub(super) async fn bind_withdrawal(
    tx: &mut TrackedTransaction<'_, Postgres>,
    input: &NewWithdrawal<'_>,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<Uuid, WriteError> {
    bind_withdrawal_inner(tx, input, None, content).await
}

/// 在 g+1 `withdrawal_only` 代上写 Withdrawal：带被取代的 Sale 与上代封闭证明引用（000137 J4）。
pub(in crate::database::consignment::recovery) async fn bind_withdrawal_after_sale(
    tx: &mut TrackedTransaction<'_, Postgres>,
    input: &NewWithdrawal<'_>,
    predecessor: &SealedPredecessor<'_>,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<Uuid, WriteError> {
    bind_withdrawal_inner(tx, input, Some(predecessor), content).await
}

async fn bind_withdrawal_inner(
    tx: &mut TrackedTransaction<'_, Postgres>,
    input: &NewWithdrawal<'_>,
    predecessor: Option<&SealedPredecessor<'_>>,
    content: &Arc<dyn FirstDispatchContentSource>,
) -> Result<Uuid, WriteError> {
    let seller_identity: Option<String> =
        sqlx::query_scalar("SELECT username FROM public.users WHERE id = $1")
            .bind(input.seller_user_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(frozen_seller_identity) = seller_identity else {
        return Err(WriteError::invariant("consignment_withdrawal_seller_missing"));
    };
    let world = parse_settlement_world(input.world_id)
        .ok_or_else(|| WriteError::invariant("consignment_withdrawal_world"))?;
    let contract_id = ConsignmentContractId::from_uuid(legacy_natural_key_uuid(
        WITHDRAWAL_CONTRACT_ID_NAMESPACE,
        &[input.origin_command_id.as_bytes()],
    ));
    let contract = ConsignmentContract::Withdrawal {
        core: ConsignmentContractCore {
            contract_id,
            origin_command_id: CommandId::from_uuid(input.origin_command_id),
            listing_id: input.listing_id,
            position_id: input.position_id,
            generation: input.generation,
            custody_equipment_id: input.equipment_id,
            world,
            canonical_eid: input.canonical_eid.to_owned(),
            season_window_id: input.season_window_id,
            seller_user_id: UserId::from_uuid(input.seller_user_id),
            frozen_seller_identity,
            created_at: input.now,
        },
        terms: WithdrawalTerms {
            intent_id: input.intent_id,
            predecessor_contract_id: predecessor
                .map(|predecessor| ConsignmentContractId::from_uuid(predecessor.sale_contract_id)),
        },
    };
    let graph = build_consignment_obligations(&contract).map_err(|error| {
        tracing::warn!(%error, "寄售撤回合同未通过校验");
        WriteError::invariant("consignment_withdrawal_contract")
    })?;
    match predecessor {
        None => write_graph_in_conn(tx, &graph).await?,
        Some(predecessor) => {
            let chain = WithdrawalChain {
                predecessor_generation: predecessor.generation,
                closure_proof_ref: predecessor.closure_proof_ref,
            };
            write_graph_with_chain(tx, &graph, Some(&chain)).await?
        }
    };
    // 不变量 1、3：撤回合同的卖方取物权与合同同事务登记。
    register_contract_rights_in_tx(tx, contract_id.as_uuid(), input.now).await?;
    // 不变量 3、4（D9 §5.3 撤回 3 “唯一工作同提”）：撤回件首轮与合同同事务；内容不可得整笔回滚。
    create_withdrawal_round_in_tx(tx, &graph, content).await?;
    Ok(contract_id.as_uuid())
}

//! 合同义务图的单事务写入：合同、子项、义务与初始履约行，同合同重放、异条款冲突。
//!
//! 保护不变量 1、2、3、6：同合同同条款重放返回已存图，同合同异条款返回 `CommandIdConflict`，
//! 任一行失败整笔回滚；COMMIT 前由 000135 的延迟触发器再核整图完整性。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ConsignmentGraphStore, StoredConsignmentGraph,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractError, ConsignmentObligation,
    ConsignmentObligationGraph, ListingEquipmentIdentity, validate_consignment_graph,
};
use auction_domain::{ConsignmentContractId, SettlementId, SettlementObligationState, UserId};
use sqlx::PgConnection;
use uuid::Uuid;

use super::policy::{PolicyColumns, funding_mode_name};
use super::source::{SourceLoadError, dispatch_world_for, load_graph, with_reference_progress};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::{LockKeys, with_command_transaction};
use crate::database::consignment::rights::register_contract_rights_in_tx;

impl ConsignmentGraphStore for DatabaseStore {
    fn write_graph(
        &self,
        graph: ConsignmentObligationGraph,
    ) -> ConsignmentFuture<'_, StoredConsignmentGraph> {
        Box::pin(async move {
            // 先在事务外校验：非法图不取任何锁、不写任何行。
            validate_consignment_graph(&graph).map_err(invalid_graph)?;
            let core = graph.contract.core();
            let identity = ListingEquipmentIdentity::new(core.world.as_str(), &core.canonical_eid)
                .map_err(|error| {
                    tracing::warn!(%error, "寄售合同装备身份无效");
                    ConsignmentError::InvalidDepositRequest
                })?;
            let sellers = [core.seller_user_id.as_uuid()];
            let equipment = [identity];
            // 合同与来源命令一一对应，所以按来源命令取全局命令锁即可串行同一合同的并发写入。
            let keys = LockKeys {
                command_id: core.origin_command_id.as_uuid(),
                sellers: &sellers,
                equipment: &equipment,
            };
            with_command_transaction(self, keys, &graph, |graph, tx| {
                Box::pin(async move {
                    let stored = write_graph_in_conn(tx.connection(), graph).await?;
                    // 不变量 1、3：端口写入与购买 / 撤回生成路径一样，权利行与合同同事务登记（幂等）。
                    let core = graph.contract.core();
                    register_contract_rights_in_tx(
                        tx.connection(),
                        core.contract_id.as_uuid(),
                        core.created_at,
                    )
                    .await?;
                    Ok(stored)
                })
            })
            .await
            .map_err(WriteError::into_application)
        })
    }
}

/// 领域整数超出数据库列范围：失败关闭，不截断。
fn overflow(error: std::num::TryFromIntError) -> ConsignmentError {
    tracing::warn!(%error, "寄售合同整数超出数据库范围");
    ConsignmentError::InvalidDepositRequest
}

fn invalid_graph(error: ConsignmentContractError) -> ConsignmentError {
    tracing::warn!(%error, "寄售义务图未通过校验");
    ConsignmentError::InvalidDepositRequest
}

/// 撤回代的前驱链路（000137 J4）：前驱处分代序号与上代封闭证明引用，随 Withdrawal 行一并写入。
pub(crate) struct WithdrawalChain<'a> {
    /// 前驱处分代序号 g；Withdrawal 所在代必为 g+1 的 `withdrawal_only` 代。
    pub(crate) predecessor_generation: u64,
    /// 上代封闭证明引用（不含装备 key 或 URL 的安全文本）。
    pub(crate) closure_proof_ref: &'a str,
}

/// 在调用方已开启的 Serializable 事务连接上写图；第 6 期的购买事务经 `&mut **tx` 嵌入。
///
/// 调用方负责持有命令、卖家与实物锁；本函数不取锁、不提交。
pub(crate) async fn write_graph_in_conn(
    conn: &mut PgConnection,
    graph: &ConsignmentObligationGraph,
) -> Result<StoredConsignmentGraph, WriteError> {
    write_graph_with_chain(conn, graph, None).await
}

/// 同 [`write_graph_in_conn`]；Withdrawal 挂在 `withdrawal_only` 代时带上前驱链路（Sale 与无链路的撤回传 `None`）。
///
/// 链路不属于合同条款，重放比较不含它：同合同重放仍只比条款（不变量 1）。
pub(crate) async fn write_graph_with_chain(
    conn: &mut PgConnection,
    graph: &ConsignmentObligationGraph,
    chain: Option<&WithdrawalChain<'_>>,
) -> Result<StoredConsignmentGraph, WriteError> {
    validate_consignment_graph(graph).map_err(invalid_graph)?;
    let obligation_ids =
        graph.obligations.iter().map(|obligation| obligation.id).collect::<Vec<_>>();
    let core = graph.contract.core();
    let contract_id = core.contract_id;

    // 合同 ID、来源命令与 (position, 代) 都是唯一键；命中任一既有合同，要么就是同一合同的同条款重放，
    // 要么是同 ID 异条款冲突，不允许再插入第二份。
    let existing: Vec<Uuid> = sqlx::query_scalar(
        "SELECT contract_id FROM public.consignment_contracts
         WHERE contract_id = $1 OR origin_command_id = $2 OR (position_id = $3 AND generation = $4)",
    )
    .bind(contract_id.as_uuid())
    .bind(core.origin_command_id.as_uuid())
    .bind(core.position_id)
    .bind(i64::try_from(core.generation).map_err(overflow)?)
    .fetch_all(&mut *conn)
    .await?;
    if !existing.is_empty() {
        if existing.iter().any(|stored| *stored != contract_id.as_uuid()) {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        return replay(conn, graph, obligation_ids).await;
    }

    insert_contract(conn, &graph.contract).await?;
    match &graph.contract {
        ConsignmentContract::Sale { core, terms } => {
            insert_sale(conn, core.contract_id.as_uuid(), terms).await?;
        }
        ConsignmentContract::Withdrawal { core, terms } => {
            let predecessor_generation = chain
                .map(|chain| i64::try_from(chain.predecessor_generation))
                .transpose()
                .map_err(overflow)?;
            sqlx::query(
                "INSERT INTO public.consignment_withdrawals
                     (contract_id, intent_id, predecessor_contract_id, predecessor_generation,
                      closure_proof_ref)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(core.contract_id.as_uuid())
            .bind(terms.intent_id)
            .bind(terms.predecessor_contract_id.map(ConsignmentContractId::as_uuid))
            .bind(predecessor_generation)
            .bind(chain.map(|chain| chain.closure_proof_ref))
            .execute(&mut *conn)
            .await?;
        }
    }
    for obligation in &graph.obligations {
        insert_obligation(conn, obligation).await?;
    }
    for obligation in &graph.obligations {
        insert_fulfillment(conn, obligation).await?;
    }
    Ok(StoredConsignmentGraph { contract_id, obligation_ids, replayed: false })
}

/// 已存图与入参逐项比较；履约推进的状态不算条款，时间按数据库微秒精度比较。
async fn replay(
    conn: &mut PgConnection,
    graph: &ConsignmentObligationGraph,
    obligation_ids: Vec<SettlementId>,
) -> Result<StoredConsignmentGraph, WriteError> {
    let contract_id = graph.contract.core().contract_id;
    let stored = match load_graph(conn, contract_id.as_uuid()).await {
        Ok(Some(stored)) => stored,
        Ok(None) => return Err(WriteError::invariant("consignment_graph_incomplete")),
        Err(SourceLoadError::Execution(error)) => return Err(error.into()),
        Err(SourceLoadError::Context(_)) => {
            return Err(WriteError::invariant("consignment_graph_corrupt"));
        }
    };
    let input = at_database_precision(graph);
    let same_obligations = input.obligations.len() == stored.obligations.len()
        && input
            .obligations
            .iter()
            .zip(&stored.obligations)
            .all(|(wanted, actual)| with_reference_progress(actual, wanted) == *wanted);
    if stored.contract != input.contract || !same_obligations {
        return Err(ConsignmentError::CommandIdConflict.into());
    }
    Ok(StoredConsignmentGraph { contract_id, obligation_ids, replayed: true })
}

/// 时间列以微秒存储；入参截断后才与读回的图可比。
fn at_database_precision(graph: &ConsignmentObligationGraph) -> ConsignmentObligationGraph {
    let mut graph = graph.clone();
    match &mut graph.contract {
        ConsignmentContract::Sale { core, terms } => {
            core.created_at = core.created_at.trunc_subsecs(6);
            terms.payment_deadline_at = terms.payment_deadline_at.trunc_subsecs(6);
            terms.funding.boundary_at = terms.funding.boundary_at.trunc_subsecs(6);
        }
        ConsignmentContract::Withdrawal { core, .. } => {
            core.created_at = core.created_at.trunc_subsecs(6);
        }
    }
    graph
}

async fn insert_contract(
    conn: &mut PgConnection,
    contract: &ConsignmentContract,
) -> Result<(), WriteError> {
    let core = contract.core();
    sqlx::query(
        "INSERT INTO public.consignment_contracts
             (contract_id, kind, origin_command_id, listing_id, position_id, generation,
              equipment_id, world_id, canonical_eid, season_window_id, seller_user_id,
              frozen_seller_identity, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(core.contract_id.as_uuid())
    .bind(contract.kind().as_str())
    .bind(core.origin_command_id.as_uuid())
    .bind(core.listing_id)
    .bind(core.position_id)
    .bind(i64::try_from(core.generation).map_err(overflow)?)
    .bind(core.custody_equipment_id)
    .bind(core.world.as_str())
    .bind(&core.canonical_eid)
    .bind(core.season_window_id)
    .bind(core.seller_user_id.as_uuid())
    .bind(&core.frozen_seller_identity)
    .bind(core.created_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_sale(
    conn: &mut PgConnection,
    contract_id: Uuid,
    terms: &auction_domain::consignment::SaleTerms,
) -> Result<(), WriteError> {
    let policy = PolicyColumns::from_policy(&terms.policy).map_err(overflow)?;
    let listing_version = i64::try_from(terms.listing_version.value()).map_err(overflow)?;
    sqlx::query(
        "INSERT INTO public.consignment_sales
             (contract_id, buyer_user_id, frozen_buyer_identity, listing_version, price, fee,
              seller_amount, pricing_version, fee_bps, payment_deadline_days, payment_deadline_at,
              reminder_limit, cod_return_limit, isekai_refusal_limit, isekai_refusal_window_days,
              funding_uid, funding_mode, funding_boundary_at, funding_boundary_source_id,
              funding_account_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
                 $19, $20)",
    )
    .bind(contract_id)
    .bind(terms.buyer_user_id.as_uuid())
    .bind(&terms.frozen_buyer_identity)
    .bind(listing_version)
    .bind(terms.price.value())
    .bind(terms.quote.fee)
    .bind(terms.quote.seller_amount)
    .bind(i16::try_from(terms.pricing_version).map_err(overflow)?)
    .bind(i16::try_from(terms.fee_bps.value()).map_err(overflow)?)
    .bind(policy.payment_deadline_days)
    .bind(terms.payment_deadline_at)
    .bind(policy.reminder_limit)
    .bind(policy.cod_return_limit)
    .bind(policy.isekai_refusal_limit)
    .bind(policy.isekai_refusal_window_days)
    .bind(&terms.funding.uid)
    .bind(funding_mode_name(terms.funding.mode))
    .bind(terms.funding.boundary_at)
    .bind(terms.funding.boundary_source_id)
    .bind(terms.funding.account_version)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_obligation(
    conn: &mut PgConnection,
    obligation: &ConsignmentObligation,
) -> Result<(), WriteError> {
    let identity = Some(&obligation.recipient_game_identity).filter(|text| !text.is_empty());
    sqlx::query(
        "INSERT INTO public.settlement_obligations
             (id, world_id, policy_version, obligation_kind, business_state, state_version,
              recipient_user_id, recipient_game_identity, amount, carrier_credit,
              prerequisite_obligation_id, contract_id, contract_kind, custody_equipment_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(obligation.id.as_uuid())
    .bind(obligation.world.as_str())
    .bind(i32::try_from(obligation.policy_version).map_err(overflow)?)
    .bind(obligation.kind.as_str())
    .bind(obligation.state.as_str())
    .bind(i64::try_from(obligation.state_version).map_err(overflow)?)
    .bind(obligation.recipient_user_id.map(UserId::as_uuid))
    .bind(identity)
    .bind(obligation.amount.value())
    .bind(i64::from(obligation.carrier_credit))
    .bind(obligation.prerequisite_id.map(SettlementId::as_uuid))
    .bind(obligation.contract_id.as_uuid())
    .bind(obligation.contract_kind.as_str())
    .bind(obligation.custody_equipment_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 初始履约行：待发送，休眠仅限 contingent 的退款义务；派发世界与领取加载的规则同源。
async fn insert_fulfillment(
    conn: &mut PgConnection,
    obligation: &ConsignmentObligation,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
             (obligation_id, execution_state, dispatch_world, delivery_operation, dormant)
         VALUES ($1, 'pending', $2, 'send', $3)",
    )
    .bind(obligation.id.as_uuid())
    .bind(dispatch_world_for(obligation.kind, obligation.world).as_str())
    .bind(obligation.state == SettlementObligationState::Contingent)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

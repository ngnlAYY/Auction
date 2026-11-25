//! 合同、义务与履约行的读取：把持久行还原为领域合同与义务图，并为领取重算图校验。
//!
//! 读序固定为 obligation → fulfillment → contract → sale/withdrawal → position（D9 §1.3 的寄售
//! 来源业务行顺序）。本模块只做普通 SELECT；行锁由领取事务在义务与履约行上取得（R5-C8），
//! 合同、sale 与 position 不加锁，最终事务的重载加锁归第 6 期。

use auction_application::consignment::ClaimIsolation;
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ConsignmentContractKind, ConsignmentObligation,
    ConsignmentObligationGraph, FeeBps, ListingPrice, ListingQuote, ListingVersion,
    SaleFundingFreeze, SaleTerms, WithdrawalTerms, build_consignment_obligations,
    validate_consignment_graph,
};
use auction_domain::{
    CommandId, ConsignmentContractId, SettlementAmount, SettlementId, SettlementObligationKind,
    SettlementObligationState, SettlementWorld, UserId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::policy::{PolicyColumns, parse_funding_mode};

/// 加载来源失败的两类：读库或解码出错（执行身份错，不转人工）与来源事实损坏（由领取事务转人工）。
#[derive(Debug)]
pub(super) enum SourceLoadError {
    Execution(sqlx::Error),
    Context(ClaimIsolation),
}

impl From<sqlx::Error> for SourceLoadError {
    fn from(error: sqlx::Error) -> Self {
        Self::Execution(error)
    }
}

const fn corrupt(reason: &'static str) -> SourceLoadError {
    SourceLoadError::Context(ClaimIsolation::Other(reason))
}

/// 领域校验失败即来源损坏；只记录固定原因与领域错误文本（不含身份与金额）。
fn corrupt_with<E: std::fmt::Display>(reason: &'static str) -> impl FnOnce(E) -> SourceLoadError {
    move |error| {
        tracing::warn!(%error, reason, "寄售合同来源校验失败");
        corrupt(reason)
    }
}

/// 领取所需的已校验来源。
#[derive(Debug)]
pub(super) struct ConsignmentObligationSource {
    pub(super) obligation: ConsignmentObligation,
    pub(super) contract: ConsignmentContract,
    pub(super) dispatch_world: SettlementWorld,
    pub(super) equipment_name: String,
    pub(super) external_equipment_id: String,
}

/// carrier、卖款与退款在 main 派发，其余在义务世界；与图写入的初始履约行同一规则。
pub(super) const fn dispatch_world_for(
    kind: SettlementObligationKind,
    world: SettlementWorld,
) -> SettlementWorld {
    match kind {
        SettlementObligationKind::WinnerPaymentCarrierCod
        | SettlementObligationKind::SellerPayout
        | SettlementObligationKind::BuyerRefund => SettlementWorld::Main,
        _ => world,
    }
}

macro_rules! obligation_select {
    () => {
        "SELECT o.id, o.world_id, o.obligation_kind, o.business_state,
        o.state_version, o.policy_version, o.recipient_user_id, o.recipient_game_identity,
        o.amount, o.carrier_credit, o.prerequisite_obligation_id, o.contract_id,
        o.contract_kind, o.custody_equipment_id
    FROM public.settlement_obligations AS o"
    };
}

fn obligation_from_row(row: &PgRow) -> Result<ConsignmentObligation, SourceLoadError> {
    let contract_id: Option<Uuid> = row.try_get("contract_id")?;
    let contract_kind: Option<String> = row.try_get("contract_kind")?;
    let custody_equipment_id: Option<Uuid> = row.try_get("custody_equipment_id")?;
    let (Some(contract_id), Some(contract_kind), Some(custody_equipment_id)) =
        (contract_id, contract_kind, custody_equipment_id)
    else {
        return Err(SourceLoadError::Context(ClaimIsolation::SourceMissing));
    };
    let world: String = row.try_get("world_id")?;
    let kind: String = row.try_get("obligation_kind")?;
    let state: String = row.try_get("business_state")?;
    let recipient_identity: Option<String> = row.try_get("recipient_game_identity")?;
    let (Some(world), Some(kind), Some(state), Some(contract_kind)) = (
        SettlementWorld::from_name(&world),
        SettlementObligationKind::from_name(&kind),
        SettlementObligationState::from_name(&state),
        ConsignmentContractKind::parse(&contract_kind),
    ) else {
        return Err(corrupt("obligation_names"));
    };
    let state_version: i64 = row.try_get("state_version")?;
    let policy_version: i32 = row.try_get("policy_version")?;
    let amount: i64 = row.try_get("amount")?;
    let carrier_credit: i64 = row.try_get("carrier_credit")?;
    let (Ok(state_version), Ok(policy_version), Ok(amount), Ok(carrier_credit)) = (
        u64::try_from(state_version),
        u32::try_from(policy_version),
        SettlementAmount::new(amount),
        u32::try_from(carrier_credit),
    ) else {
        return Err(corrupt("obligation_numbers"));
    };
    Ok(ConsignmentObligation {
        id: SettlementId::from_uuid(row.try_get("id")?),
        contract_id: ConsignmentContractId::from_uuid(contract_id),
        contract_kind,
        world,
        kind,
        state,
        state_version,
        policy_version,
        recipient_user_id: row
            .try_get::<Option<Uuid>, _>("recipient_user_id")?
            .map(UserId::from_uuid),
        recipient_game_identity: recipient_identity.unwrap_or_default(),
        amount,
        carrier_credit,
        custody_equipment_id,
        prerequisite_id: row
            .try_get::<Option<Uuid>, _>("prerequisite_obligation_id")?
            .map(SettlementId::from_uuid),
    })
}

/// 读取合同头与对应子项，还原为领域合同；合同或子项缺失返回 `Ok(None)`。
pub(super) async fn load_contract(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<ConsignmentContract>, SourceLoadError> {
    let Some(head) = sqlx::query(
        "SELECT kind, origin_command_id, listing_id, position_id, generation, equipment_id,
                world_id, canonical_eid, season_window_id, seller_user_id,
                frozen_seller_identity, created_at
         FROM public.consignment_contracts WHERE contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let kind: String = head.try_get("kind")?;
    let world: String = head.try_get("world_id")?;
    let generation: i64 = head.try_get("generation")?;
    let (Some(kind), Some(world), Ok(generation)) = (
        ConsignmentContractKind::parse(&kind),
        SettlementWorld::from_name(&world),
        u64::try_from(generation),
    ) else {
        return Err(corrupt("contract_head"));
    };
    let core = ConsignmentContractCore {
        contract_id: ConsignmentContractId::from_uuid(contract_id),
        origin_command_id: CommandId::from_uuid(head.try_get("origin_command_id")?),
        listing_id: head.try_get("listing_id")?,
        position_id: head.try_get("position_id")?,
        generation,
        custody_equipment_id: head.try_get("equipment_id")?,
        world,
        canonical_eid: head.try_get("canonical_eid")?,
        season_window_id: head.try_get("season_window_id")?,
        seller_user_id: UserId::from_uuid(head.try_get("seller_user_id")?),
        frozen_seller_identity: head.try_get("frozen_seller_identity")?,
        created_at: head.try_get("created_at")?,
    };
    let contract = match kind {
        ConsignmentContractKind::Sale => {
            let Some(terms) = load_sale_terms(conn, contract_id).await? else {
                return Ok(None);
            };
            ConsignmentContract::Sale { core, terms }
        }
        ConsignmentContractKind::Withdrawal => {
            let Some(terms) = load_withdrawal_terms(conn, contract_id).await? else {
                return Ok(None);
            };
            ConsignmentContract::Withdrawal { core, terms }
        }
    };
    contract.validate().map_err(corrupt_with("contract_terms"))?;
    Ok(Some(contract))
}

async fn load_sale_terms(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<SaleTerms>, SourceLoadError> {
    let Some(row) = sqlx::query(
        "SELECT buyer_user_id, frozen_buyer_identity, listing_version, price, fee, seller_amount,
                pricing_version, fee_bps, payment_deadline_days, payment_deadline_at,
                reminder_limit, cod_return_limit, isekai_refusal_limit,
                isekai_refusal_window_days, funding_uid, funding_mode, funding_boundary_at,
                funding_boundary_source_id, funding_account_version
         FROM public.consignment_sales WHERE contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let listing_version: i64 = row.try_get("listing_version")?;
    let price: i64 = row.try_get("price")?;
    let fee_bps: i16 = row.try_get("fee_bps")?;
    let pricing_version: i16 = row.try_get("pricing_version")?;
    let funding_mode: String = row.try_get("funding_mode")?;
    let (Some(listing_version), Ok(price), Ok(fee_bps), Ok(pricing_version), Some(mode)) = (
        u64::try_from(listing_version).ok().and_then(|value| ListingVersion::new(value).ok()),
        ListingPrice::new(price),
        FeeBps::new(i64::from(fee_bps)),
        u16::try_from(pricing_version),
        parse_funding_mode(&funding_mode),
    ) else {
        return Err(corrupt("sale_terms"));
    };
    let policy = PolicyColumns {
        payment_deadline_days: row.try_get("payment_deadline_days")?,
        reminder_limit: row.try_get("reminder_limit")?,
        cod_return_limit: row.try_get("cod_return_limit")?,
        isekai_refusal_limit: row.try_get("isekai_refusal_limit")?,
        isekai_refusal_window_days: row.try_get("isekai_refusal_window_days")?,
    }
    .into_policy()
    .ok_or(corrupt("sale_policy"))?;
    Ok(Some(SaleTerms {
        buyer_user_id: UserId::from_uuid(row.try_get("buyer_user_id")?),
        frozen_buyer_identity: row.try_get("frozen_buyer_identity")?,
        listing_version,
        price,
        fee_bps,
        quote: ListingQuote {
            fee: row.try_get("fee")?,
            seller_amount: row.try_get("seller_amount")?,
        },
        pricing_version,
        payment_deadline_at: row.try_get("payment_deadline_at")?,
        policy,
        funding: SaleFundingFreeze {
            uid: row.try_get("funding_uid")?,
            mode,
            boundary_at: row.try_get("funding_boundary_at")?,
            boundary_source_id: row.try_get("funding_boundary_source_id")?,
            account_version: row.try_get("funding_account_version")?,
        },
    }))
}

async fn load_withdrawal_terms(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<WithdrawalTerms>, SourceLoadError> {
    let Some(row) = sqlx::query(
        "SELECT intent_id, predecessor_contract_id
         FROM public.consignment_withdrawals WHERE contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(WithdrawalTerms {
        intent_id: row.try_get("intent_id")?,
        predecessor_contract_id: row
            .try_get::<Option<Uuid>, _>("predecessor_contract_id")?
            .map(ConsignmentContractId::from_uuid),
    }))
}

/// 合同的全部义务，按义务 ID 排序；调用方按需重排。
pub(super) async fn load_obligations(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Vec<ConsignmentObligation>, SourceLoadError> {
    let rows =
        sqlx::query(concat!(obligation_select!(), " WHERE o.contract_id = $1 ORDER BY o.id"))
            .bind(contract_id)
            .fetch_all(&mut *conn)
            .await?;
    rows.iter().map(obligation_from_row).collect()
}

/// 状态与状态版本随履约推进而变化，不属于合同冻结事实；比较前统一到参照值。
pub(super) fn with_reference_progress(
    actual: &ConsignmentObligation,
    reference: &ConsignmentObligation,
) -> ConsignmentObligation {
    ConsignmentObligation {
        state: reference.state,
        state_version: reference.state_version,
        ..actual.clone()
    }
}

/// 读取并还原已存图，按合同重建顺序排列；缺合同、缺义务或多义务都返回 `Ok(None)` 由调用方判定。
pub(super) async fn load_graph(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<Option<ConsignmentObligationGraph>, SourceLoadError> {
    let Some(contract) = load_contract(conn, contract_id).await? else {
        return Ok(None);
    };
    let expected = build_consignment_obligations(&contract).map_err(corrupt_with("graph_shape"))?;
    let stored = load_obligations(conn, contract_id).await?;
    let obligations = expected
        .obligations
        .iter()
        .filter_map(|reference| {
            stored
                .iter()
                .find(|actual| actual.id == reference.id)
                .map(|actual| with_reference_progress(actual, reference))
        })
        .collect::<Vec<_>>();
    if obligations.len() != stored.len() || obligations.len() != expected.obligations.len() {
        return Ok(None);
    }
    Ok(Some(ConsignmentObligationGraph { contract, obligations }))
}

/// 在义务与履约行已被领取事务锁住之后加载并校验来源。
///
/// 任何来源事实与合同不符都返回 `Context`，由调用方转人工对账而不发送（不变量 6）。
pub(super) async fn load_source_for_claim(
    conn: &mut PgConnection,
    obligation_id: Uuid,
) -> Result<ConsignmentObligationSource, SourceLoadError> {
    let Some(row) = sqlx::query(concat!(obligation_select!(), " WHERE o.id = $1"))
        .bind(obligation_id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Err(SourceLoadError::Context(ClaimIsolation::SourceMissing));
    };
    let obligation = obligation_from_row(&row)?;
    if obligation.state != SettlementObligationState::Ready {
        return Err(corrupt("obligation_not_ready"));
    }

    let fulfillment = sqlx::query(
        "SELECT dispatch_world, delivery_operation
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(fulfillment) = fulfillment else {
        return Err(SourceLoadError::Context(ClaimIsolation::SourceMissing));
    };
    let dispatch_world: String = fulfillment.try_get("dispatch_world")?;
    let operation: String = fulfillment.try_get("delivery_operation")?;
    let dispatch_world = SettlementWorld::from_name(&dispatch_world)
        .filter(|world| *world == dispatch_world_for(obligation.kind, obligation.world))
        .ok_or(SourceLoadError::Context(ClaimIsolation::WorldMismatch))?;
    if operation != "send" {
        return Err(corrupt("fulfillment_operation"));
    }

    let contract_id = obligation.contract_id.as_uuid();
    let Some(contract) = load_contract(conn, contract_id).await? else {
        return Err(SourceLoadError::Context(ClaimIsolation::SourceMissing));
    };
    let core = contract.core();
    if contract.kind() != obligation.contract_kind {
        return Err(corrupt("contract_kind"));
    }
    if core.world != obligation.world {
        return Err(SourceLoadError::Context(ClaimIsolation::WorldMismatch));
    }
    if core.custody_equipment_id != obligation.custody_equipment_id {
        return Err(corrupt("custody_equipment"));
    }

    let (equipment_name, external_equipment_id) =
        check_position_and_snapshot(conn, &contract).await?;
    check_graph(conn, &contract, &obligation).await?;
    Ok(ConsignmentObligationSource {
        obligation,
        contract,
        dispatch_world,
        equipment_name,
        external_equipment_id,
    })
}

/// position 身份必须与合同逐项一致，并取同事务内冻结的装备名称。
async fn check_position_and_snapshot(
    conn: &mut PgConnection,
    contract: &ConsignmentContract,
) -> Result<(String, String), SourceLoadError> {
    let core = contract.core();
    let Some(position) = sqlx::query(
        "SELECT listing_id, seller_user_id, equipment_id, world_id, canonical_eid
         FROM public.consignment_custody_positions WHERE id = $1",
    )
    .bind(core.position_id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(SourceLoadError::Context(ClaimIsolation::SourceMissing));
    };
    let world: String = position.try_get("world_id")?;
    if SettlementWorld::from_name(&world) != Some(core.world) {
        return Err(SourceLoadError::Context(ClaimIsolation::WorldMismatch));
    }
    let eid: String = position.try_get("canonical_eid")?;
    if position.try_get::<Uuid, _>("listing_id")? != core.listing_id
        || position.try_get::<Uuid, _>("seller_user_id")? != core.seller_user_id.as_uuid()
        || position.try_get::<Uuid, _>("equipment_id")? != core.custody_equipment_id
        || eid != core.canonical_eid
    {
        return Err(corrupt("position_identity"));
    }
    let Some(snapshot) = sqlx::query(
        "SELECT name, external_equipment_id FROM public.consignment_snapshots
         WHERE listing_id = $1 AND world_id = $2 AND external_equipment_id = $3
         ORDER BY generation DESC LIMIT 1",
    )
    .bind(core.listing_id)
    .bind(&world)
    .bind(&core.canonical_eid)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(SourceLoadError::Context(ClaimIsolation::SourceMissing));
    };
    Ok((snapshot.try_get("name")?, snapshot.try_get("external_equipment_id")?))
}

/// 重算整图并核对被领取的义务；状态随履约推进，按重建值比较冻结部分。
async fn check_graph(
    conn: &mut PgConnection,
    contract: &ConsignmentContract,
    claimed: &ConsignmentObligation,
) -> Result<(), SourceLoadError> {
    let expected = build_consignment_obligations(contract).map_err(corrupt_with("graph_shape"))?;
    let stored = load_obligations(conn, contract.core().contract_id.as_uuid()).await?;
    let mut obligations = Vec::with_capacity(expected.obligations.len());
    for reference in &expected.obligations {
        if let Some(actual) = stored.iter().find(|actual| actual.id == reference.id) {
            obligations.push(with_reference_progress(actual, reference));
        }
    }
    let graph = ConsignmentObligationGraph { contract: contract.clone(), obligations };
    if validate_consignment_graph(&graph).is_ok() && stored.len() == expected.obligations.len() {
        return Ok(());
    }
    Err(SourceLoadError::Context(classify_mismatch(claimed, &expected.obligations)))
}

/// 以被领取义务相对重建结果的差异归类隔离原因。
fn classify_mismatch(
    claimed: &ConsignmentObligation,
    expected: &[ConsignmentObligation],
) -> ClaimIsolation {
    let Some(reference) = expected.iter().find(|reference| reference.kind == claimed.kind) else {
        return ClaimIsolation::Other("obligation_kind");
    };
    if reference.world != claimed.world {
        ClaimIsolation::WorldMismatch
    } else if reference.amount != claimed.amount
        || reference.carrier_credit != claimed.carrier_credit
    {
        ClaimIsolation::AmountMismatch
    } else if reference.recipient_user_id != claimed.recipient_user_id
        || reference.recipient_game_identity != claimed.recipient_game_identity
    {
        ClaimIsolation::RecipientMismatch
    } else {
        ClaimIsolation::Other("graph")
    }
}

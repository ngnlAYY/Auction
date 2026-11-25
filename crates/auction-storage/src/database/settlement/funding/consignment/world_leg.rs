//! 付款义务的世界与收款腿互校：读出义务的世界与种类，交给应用层的同一条规则（`CodLeg::check_world`，
//! 与 `CertifyCod::check_leg` 同源）。
//!
//! 保护不变量 6：载体本金只属于 isekai 的载体付款义务，main 的原件付款没有载体本金；世界与腿不符失败关闭。

use auction_application::settlement::{CodLeg, FundingError};
use auction_domain::SettlementWorld;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage;

/// 义务的收款腿，并校验它与义务所在世界对应；非付款义务或未知世界失败关闭。
pub(super) async fn checked_payment_leg(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    contract_id: Uuid,
) -> Result<CodLeg, FundingError> {
    let row = sqlx::query(
        r#"SELECT obligation_kind, world_id
           FROM public.settlement_obligations
           WHERE id=$1 AND contract_id=$2"#,
    )
    .bind(obligation_id)
    .bind(contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Invalid)?;
    let leg = match row.try_get::<String, _>("obligation_kind").map_err(storage)?.as_str() {
        "winner_payment_carrier_cod" => CodLeg::Carrier,
        "winner_equipment_cod" => CodLeg::Original,
        _ => return Err(FundingError::Invalid),
    };
    let world = match row.try_get::<String, _>("world_id").map_err(storage)?.as_str() {
        "main" => SettlementWorld::Main,
        "isekai" => SettlementWorld::Isekai,
        _ => return Err(FundingError::Unproven),
    };
    leg.check_world(world)?;
    Ok(leg)
}

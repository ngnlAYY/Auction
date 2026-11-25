//! 退件接管事务的预读与锁集合：命令 → 买家用户 → 规范实物 → K（D9 §1.3、R5 §3.8）。
//!
//! 预读只用来决定取哪些会话锁，不具授权含义；锁内由各写端按同一事实重读重判。

use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::consignment::OriginalMailKey;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::identity::parse_world;
use crate::database::mail::originals::invariant_decoding;

/// 一次接管事务要取的锁所依据的预读结果。
#[derive(Debug, Clone)]
pub(crate) struct LockPlan {
    /// 成交买家；拒收事实写入与购买共用这把用户锁。提款合同没有买家。
    pub(super) buyer: Option<Uuid>,
    /// 合同的规范实物。
    pub(super) equipment: ListingEquipmentIdentity,
}

impl LockPlan {
    /// 会话锁里的用户集合。
    pub(crate) fn sellers(&self) -> Vec<Uuid> {
        self.buyer.into_iter().collect()
    }

    pub(crate) fn equipment(&self) -> Vec<ListingEquipmentIdentity> {
        vec![self.equipment.clone()]
    }
}

fn plan_of(row: &sqlx::postgres::PgRow) -> Result<LockPlan, WriteError> {
    let equipment = ListingEquipmentIdentity::new(
        &row.try_get::<String, _>("world_id")?,
        &row.try_get::<String, _>("canonical_eid")?,
    )
    .map_err(invariant_decoding("consignment_return_equipment"))?;
    Ok(LockPlan { buyer: row.try_get("buyer_user_id")?, equipment })
}

/// 直接接管：由应用层从 RTS 主题证明的 (义务, 原轮) 预读合同的买家与实物。
pub(crate) async fn pre_read_by_round(
    connection: &mut PgConnection,
    obligation_id: Uuid,
    round: u32,
) -> Result<Option<LockPlan>, WriteError> {
    let row = sqlx::query(
        "SELECT contract.world_id, contract.canonical_eid, sale.buyer_user_id
         FROM public.consignment_dispatch_rounds AS round
         JOIN public.consignment_contracts AS contract ON contract.contract_id = round.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE round.obligation_id = $1 AND round.round = $2",
    )
    .bind(obligation_id)
    .bind(i32::try_from(round)?)
    .fetch_optional(&mut *connection)
    .await?;
    row.as_ref().map(plan_of).transpose()
}

/// 隔离解除：由 RTS 正文末行的派发引用（轮 ID）预读合同的买家与实物。
pub(crate) async fn pre_read_by_round_id(
    connection: &mut PgConnection,
    round_id: Uuid,
) -> Result<Option<LockPlan>, WriteError> {
    let row = sqlx::query(
        "SELECT contract.world_id, contract.canonical_eid, sale.buyer_user_id
         FROM public.consignment_dispatch_rounds AS round
         JOIN public.consignment_contracts AS contract ON contract.contract_id = round.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE round.round_id = $1",
    )
    .bind(round_id)
    .fetch_optional(&mut *connection)
    .await?;
    row.as_ref().map(plan_of).transpose()
}

/// 起写与消费：由接管行预读 K 与锁集合。
pub(super) async fn pre_read_by_intake(
    connection: &mut PgConnection,
    intake_id: Uuid,
) -> Result<Option<(OriginalMailKey, LockPlan)>, WriteError> {
    let row = sqlx::query(
        "SELECT intake.bot_uid, intake.world_id AS mail_world, intake.external_mid,
                contract.world_id, contract.canonical_eid, sale.buyer_user_id
         FROM public.consignment_return_intakes AS intake
         JOIN public.consignment_contracts AS contract ON contract.contract_id = intake.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE intake.id = $1",
    )
    .bind(intake_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let mail_world: String = row.try_get("mail_world")?;
    let key = OriginalMailKey::new(
        &row.try_get::<String, _>("bot_uid")?,
        parse_world(&mail_world)?,
        &row.try_get::<String, _>("external_mid")?,
    )
    .map_err(invariant_decoding("consignment_intake_mail_key"))?;
    Ok(Some((key, plan_of(&row)?)))
}

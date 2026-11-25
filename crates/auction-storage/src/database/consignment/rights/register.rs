//! 权利登记：合同成立与整封接受时，在同一事务内登记受益人的权利行。
//!
//! 保护不变量 1、2、3、6：登记与合同图 / 取得证明同事务提交，任何一步失败整笔回滚；同合同同角色
//! 只有一行权利（`right_id` 由领域 [`right_id`] 派生，与迁移 000137 的 `c7_right_id` 逐字节一致），
//! 重复登记返回既有行，受益人 / 种类 / 冻结身份与既有行不符则失败关闭，绝不改写。
//!
//! 定位符摘要取不可逆的占位值（任何合法定位符都不与之对应），与迁移回填同口径：持有人经本人页或
//! 冻结邮箱走找回流程取得挑战码，定位符本身不在登记时发放。

use auction_domain::consignment::rights::{RightKind, RightRole, right_id};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::claim::refresh_right_state_in_tx;
use crate::database::consignment::commands::errors::WriteError;

/// 登记点占位定位符的域前缀；与迁移回填的前缀不同，便于区分来源。
const PLACEHOLDER_LOCATOR_PREFIX: &[u8] = b"auction.consignment.right.registered-locator.v1\0";

/// 一项待登记的合同权利。
struct PlannedRight {
    role: RightRole,
    kind: RightKind,
    beneficiary_user_id: Uuid,
    frozen_identity: String,
}

/// 在合同已写入的事务里登记它的权利：Sale 的买方 `continue_order` 与卖方 `claim_payment`，
/// Withdrawal 的卖方 `claim_equipment`。调用方持有合同与来源命令的锁，且义务图已写入。
pub(crate) async fn register_contract_rights_in_tx(
    connection: &mut PgConnection,
    contract_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let row = sqlx::query(
        "SELECT contract.kind, contract.world_id, contract.seller_user_id,
                contract.frozen_seller_identity, contract.origin_command_id,
                sale.buyer_user_id, sale.frozen_buyer_identity
         FROM public.consignment_contracts AS contract
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
         WHERE contract.contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| WriteError::invariant("consignment_right_contract_missing"))?;
    let kind: String = row.try_get("kind")?;
    let world_id: String = row.try_get("world_id")?;
    let origin_command_id: Uuid = row.try_get("origin_command_id")?;
    let seller = (row.try_get("seller_user_id")?, row.try_get("frozen_seller_identity")?);
    let buyer: (Option<Uuid>, Option<String>) =
        (row.try_get("buyer_user_id")?, row.try_get("frozen_buyer_identity")?);
    let mut planned = Vec::with_capacity(2);
    match (kind.as_str(), buyer) {
        ("sale", (Some(beneficiary_user_id), Some(frozen_identity))) => {
            planned.push(PlannedRight {
                role: RightRole::Buyer,
                kind: RightKind::ContinueOrder,
                beneficiary_user_id,
                frozen_identity,
            });
            planned.push(seller_right(RightKind::ClaimPayment, seller));
        }
        ("withdrawal", _) => planned.push(seller_right(RightKind::ClaimEquipment, seller)),
        _ => return Err(WriteError::invariant("consignment_right_contract_shape")),
    }
    for plan in planned {
        let planned_id = right_id(contract_id, plan.role);
        sqlx::query(
            "INSERT INTO public.consignment_rights
                (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
                 contract_id, position_id, created_command_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, $8, $9)
             ON CONFLICT (right_id) DO NOTHING",
        )
        .bind(planned_id)
        .bind(placeholder_locator_digest(planned_id).as_slice())
        .bind(plan.beneficiary_user_id)
        .bind(&plan.frozen_identity)
        .bind(&world_id)
        .bind(plan.kind.as_str())
        .bind(contract_id)
        .bind(origin_command_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        // 不变量 6：既有行（重放或回填）必须与本次计划逐项一致，否则失败关闭。
        let stored = sqlx::query(
            "SELECT beneficiary_user_id, frozen_identity, kind, contract_id
             FROM public.consignment_rights WHERE right_id = $1",
        )
        .bind(planned_id)
        .fetch_one(&mut *connection)
        .await?;
        let same = stored.try_get::<Uuid, _>("beneficiary_user_id")? == plan.beneficiary_user_id
            && stored.try_get::<String, _>("frozen_identity")? == plan.frozen_identity
            && stored.try_get::<String, _>("kind")? == plan.kind.as_str()
            && stored.try_get::<Option<Uuid>, _>("contract_id")? == Some(contract_id);
        if !same {
            return Err(WriteError::invariant("consignment_right_identity_conflict"));
        }
        refresh_right_state_in_tx(connection, planned_id, now).await?;
    }
    Ok(())
}

fn seller_right(kind: RightKind, seller: (Uuid, String)) -> PlannedRight {
    PlannedRight {
        role: RightRole::Seller,
        kind,
        beneficiary_user_id: seller.0,
        frozen_identity: seller.1,
    }
}

/// 整封接受后登记卖家对代管件的 `withdraw_held` 权利（目标是 position，没有合同）。
///
/// 不变量 1：同卖家同 position 只有一行（`c7_right_identity_uq`），重放与并发都落在既有行上。
/// 位置权利没有合同可派生 ID，使用随机 UUID；冻结身份取接受时卖家的当前游戏身份。
pub(crate) async fn register_position_right_in_tx(
    connection: &mut PgConnection,
    position_id: Uuid,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    register_seller_position_right(
        connection,
        position_id,
        RightKind::WithdrawHeld,
        command_id,
        now,
    )
    .await
}

/// 损失案件开案时登记卖家对该 position 的 `loss_case` 权利（只读：任何状态版本的动作子集恒为空）。
///
/// 不变量 1、3：与开案同事务，同卖家同 position 只有一行（`c7_right_identity_uq`），重放不增行。
pub(crate) async fn register_loss_case_right_in_tx(
    connection: &mut PgConnection,
    position_id: Uuid,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    register_seller_position_right(connection, position_id, RightKind::LossCase, command_id, now)
        .await
}

/// 以 position 为目标登记卖家权利的共同写法。
async fn register_seller_position_right(
    connection: &mut PgConnection,
    position_id: Uuid,
    kind: RightKind,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let row = sqlx::query(
        "SELECT position.seller_user_id, position.world_id, seller.username
         FROM public.consignment_custody_positions AS position
         JOIN public.users AS seller ON seller.id = position.seller_user_id
         WHERE position.id = $1",
    )
    .bind(position_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| WriteError::invariant("consignment_right_position_missing"))?;
    let seller_user_id: Uuid = row.try_get("seller_user_id")?;
    let new_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.consignment_rights
            (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
             contract_id, position_id, created_command_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $9, NULL, $6, $7, $8)
         ON CONFLICT ON CONSTRAINT c7_right_identity_uq DO NOTHING",
    )
    .bind(new_id)
    .bind(placeholder_locator_digest(new_id).as_slice())
    .bind(seller_user_id)
    .bind(row.try_get::<String, _>("username")?)
    .bind(row.try_get::<String, _>("world_id")?)
    .bind(position_id)
    .bind(command_id)
    .bind(now)
    .bind(kind.as_str())
    .execute(&mut *connection)
    .await?;
    let stored: Uuid = sqlx::query_scalar(
        "SELECT right_id FROM public.consignment_rights
         WHERE beneficiary_user_id = $1 AND kind = $3
           AND contract_id IS NULL AND position_id = $2",
    )
    .bind(seller_user_id)
    .bind(position_id)
    .bind(kind.as_str())
    .fetch_one(&mut *connection)
    .await?;
    refresh_right_state_in_tx(connection, stored, now).await?;
    Ok(())
}

/// 不可逆占位摘要：无任何 `R1-` 文本的 SHA-256 等于它，所以不能被当作定位符使用。
fn placeholder_locator_digest(right: Uuid) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(PLACEHOLDER_LOCATOR_PREFIX);
    hash.update(right.as_bytes());
    hash.finalize().into()
}

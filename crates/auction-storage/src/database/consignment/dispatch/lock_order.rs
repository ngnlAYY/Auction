//! 派发事务的锁序：规范实物会话锁的预读，以及事务内“挂单 → position”行锁。
//!
//! 保护不变量 2：最终事务在命令锁之后取得与购买相同的卖家与规范实物会话锁，再按合同顺序
//! （挂单 → position → 履约行 → 凭据 → 投递）取行锁，使起写与按该顺序加锁的购买、收款、退件
//! 与 position 迁移互斥且不成环（D9 §1.3、R5 §3.8）。合同行对运行账号只读不可锁（无 UPDATE 权限，
//! 且只追加），所以链从挂单开始。

use std::time::Duration;

use auction_application::consignment::ConsignmentError;
use auction_domain::consignment::ListingEquipmentIdentity;
use sqlx::{PgConnection, PgExecutor};
use tokio::time::error::Elapsed;
use tokio::time::timeout;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;

/// 合同的卖家与规范实物身份；与购买事务的会话锁键同源。
pub(in crate::database::consignment) struct ContractLockIdentity {
    pub(in crate::database::consignment) sellers: Vec<Uuid>,
    pub(in crate::database::consignment) equipment: Vec<ListingEquipmentIdentity>,
}

const BY_CONTRACT: &str = "SELECT seller_user_id, world_id, canonical_eid
     FROM public.consignment_contracts WHERE contract_id = $1";
const BY_DELIVERY: &str =
    "SELECT contract.seller_user_id, contract.world_id, contract.canonical_eid
     FROM public.consignment_dispatch_rounds AS round
     JOIN public.consignment_contracts AS contract ON contract.contract_id = round.contract_id
     WHERE round.delivery_id = $1";

async fn read_identity<'e>(
    executor: impl PgExecutor<'e>,
    sql: &'static str,
    key: Uuid,
) -> Result<ContractLockIdentity, WriteError> {
    let row: Option<(Uuid, String, String)> =
        sqlx::query_as(sql).bind(key).fetch_optional(executor).await?;
    let Some((seller, world_id, canonical_eid)) = row else {
        // 合同或轮尚不存在：没有可锁的实物，业务 SQL 会按来源不符确定拒绝。
        return Ok(ContractLockIdentity { sellers: Vec::new(), equipment: Vec::new() });
    };
    let equipment = ListingEquipmentIdentity::new(&world_id, &canonical_eid).map_err(|error| {
        tracing::warn!(%error, "寄售合同实物身份无效");
        WriteError::invariant("consignment_dispatch_contract_identity")
    })?;
    Ok(ContractLockIdentity { sellers: vec![seller], equipment: vec![equipment] })
}

/// 锁外预读：按合同 ID 取锁键所需身份。
pub(in crate::database::consignment) async fn pre_read_by_contract(
    store: &DatabaseStore,
    contract_id: Uuid,
) -> Result<ContractLockIdentity, WriteError> {
    timeout(Duration::from_secs(5), read_identity(store.pool(), BY_CONTRACT, contract_id))
        .await
        .map_err(|Elapsed { .. }| ConsignmentError::Unavailable)?
}

/// 锁外预读：按投递取其轮所属合同的锁键身份。
pub(super) async fn pre_read_by_delivery(
    store: &DatabaseStore,
    delivery_id: Uuid,
) -> Result<ContractLockIdentity, WriteError> {
    timeout(Duration::from_secs(5), read_identity(store.pool(), BY_DELIVERY, delivery_id))
        .await
        .map_err(|Elapsed { .. }| ConsignmentError::Unavailable)?
}

/// 事务内重读，供 `ensure_lock_set` 与预读核对。
pub(in crate::database::consignment) async fn read_by_contract(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<ContractLockIdentity, WriteError> {
    read_identity(connection, BY_CONTRACT, contract_id).await
}

/// 事务内重读（按投递）。
pub(super) async fn read_by_delivery(
    connection: &mut PgConnection,
    delivery_id: Uuid,
) -> Result<ContractLockIdentity, WriteError> {
    read_identity(connection, BY_DELIVERY, delivery_id).await
}

/// 按合同顺序先锁挂单再锁 position；合同不存在返回 `false`，不取任何行锁。
pub(in crate::database::consignment) async fn lock_listing_and_position(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<bool, WriteError> {
    let ids: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT listing_id, position_id FROM public.consignment_contracts WHERE contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((listing_id, position_id)) = ids else {
        return Ok(false);
    };
    sqlx::query("SELECT 1 FROM public.consignment_listings WHERE id = $1 FOR UPDATE")
        .bind(listing_id)
        .fetch_optional(&mut *connection)
        .await?;
    sqlx::query("SELECT 1 FROM public.consignment_custody_positions WHERE id = $1 FOR UPDATE")
        .bind(position_id)
        .fetch_optional(&mut *connection)
        .await?;
    Ok(true)
}

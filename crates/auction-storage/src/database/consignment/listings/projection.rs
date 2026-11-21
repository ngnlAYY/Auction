//! 挂单锁键预读、版本校验与持久投影。

use std::time::Duration;

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection};
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingPrice, ListingReason, ListingSnapshot, ListingVersion,
    UnknownListingReason,
};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use tokio::time::error::Elapsed;
use tokio::time::timeout;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::{WriteError, invalid_projection};

/// 只由不可变字段组成的锁键。读取结果没有授权、资格或状态含义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListingLockIdentity {
    /// 固定卖家的本站 UUID。
    pub(crate) seller_user_id: Uuid,
    /// 固定的规范世界与 EID。
    pub(crate) equipment: ListingEquipmentIdentity,
}

impl ListingLockIdentity {
    fn from_row(row: &PgRow) -> Result<Self, WriteError> {
        Ok(Self {
            seller_user_id: row.try_get("seller_user_id")?,
            equipment: ListingEquipmentIdentity::new(
                row.try_get("world_id")?,
                row.try_get("external_equipment_id")?,
            )
            .map_err(invalid_projection)?,
        })
    }
}

/// 锁内读取的挂单，领域迁移所需数据在 `snapshot`。
#[derive(Debug)]
pub(crate) struct LockedListing {
    /// 挂单 UUID。
    pub(crate) listing_id: Uuid,
    /// 不可变卖家及装备锁键。
    pub(crate) identity: ListingLockIdentity,
    /// 用于领域迁移的当前投影。
    pub(crate) snapshot: ListingSnapshot,
    /// 本单所关联的安全装备快照。
    pub(crate) current_snapshot_id: Option<Uuid>,
    #[allow(dead_code, reason = "由后续任务 2D / 2F 读取锁内投影")]
    /// 登记冻结的费率。
    pub(crate) fee_bps: FeeBps,
    /// 登记时固定的服务代次。
    pub(crate) service_epoch: i64,
    /// 登记时已见的拉黑历史事实数。
    pub(crate) blacklist_count_at_register: i64,
    /// 数据库登记时刻。
    pub(crate) created_at: Timestamp,
    #[allow(dead_code, reason = "由后续任务 2D / 2F 读取锁内投影")]
    /// 最近一次投影变动的数据库时刻。
    pub(crate) updated_at: Timestamp,
}

impl LockedListing {
    fn from_row(row: PgRow) -> Result<Self, WriteError> {
        let state: &str = row.try_get("state")?;
        let mode: &str = row.try_get("mode")?;
        let reason: Option<&str> = row.try_get("reason_code")?;
        Ok(Self {
            listing_id: row.try_get("id")?,
            identity: ListingLockIdentity::from_row(&row)?,
            snapshot: ListingSnapshot {
                mode: mode.parse().map_err(invalid_projection)?,
                state: state.parse().map_err(invalid_projection)?,
                state_version: ListingVersion::new(
                    row.try_get::<i64, _>("state_version")?.cast_unsigned(),
                )
                .map_err(invalid_projection)?,
                price: ListingPrice::new(row.try_get("price")?).map_err(invalid_projection)?,
                expires_at: row.try_get("expires_at")?,
                reason: reason.map(ListingReason::parse).transpose().map_err(
                    |UnknownListingReason| WriteError::invariant("consignment_listing_reason"),
                )?,
            },
            current_snapshot_id: row.try_get("current_snapshot_id")?,
            fee_bps: FeeBps::new(i64::from(row.try_get::<i16, _>("fee_bps")?))
                .map_err(invalid_projection)?,
            service_epoch: row.try_get("service_epoch")?,
            blacklist_count_at_register: row.try_get("blacklist_count_at_register")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    /// 不变量 6：强制清退及归属检查完成后才比较调用方预期版本。
    #[allow(dead_code, reason = "由后续任务 2D 调用")]
    pub(crate) fn require_version(
        &self,
        expected_version: ListingVersion,
    ) -> Result<(), ListingBusinessRejection> {
        if self.snapshot.state_version == expected_version {
            Ok(())
        } else {
            Err(ListingBusinessRejection::StaleVersion)
        }
    }
}

/// 在取会话锁前预读锁键。即使目标不存在，调用方也须拿命令锁并完成仲裁。
pub(crate) async fn pre_read_listing_identity(
    store: &DatabaseStore,
    listing_id: Uuid,
) -> Result<Option<ListingLockIdentity>, WriteError> {
    let row = timeout(
        Duration::from_secs(5),
        sqlx::query(
            "SELECT seller_user_id, world_id, external_equipment_id
             FROM public.consignment_listings WHERE id = $1",
        )
        .bind(listing_id)
        .fetch_optional(store.pool()),
    )
    .await
    .map_err(|Elapsed { .. }| ConsignmentError::Unavailable)??;
    row.as_ref().map(ListingLockIdentity::from_row).transpose()
}

/// 在已取得相应会话锁、锁定当前资格行后，一次按 UUID 升序获取涉及的挂单行锁。
pub(crate) async fn lock_listings(
    connection: &mut PgConnection,
    listing_ids: &[Uuid],
) -> Result<Vec<LockedListing>, WriteError> {
    let rows = sqlx::query(
        "SELECT id, seller_user_id, world_id, external_equipment_id, mode, state,
                state_version, price, current_snapshot_id, fee_bps, service_epoch,
                blacklist_count_at_register, created_at, updated_at, expires_at, reason_code
         FROM public.consignment_listings WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(listing_ids)
    .fetch_all(connection)
    .await?;
    rows.into_iter().map(LockedListing::from_row).collect()
}

/// 登记首次写入的不可变身份和冻结策略。
pub(crate) struct NewListing<'a> {
    /// 挂单 UUID。
    pub(crate) listing_id: Uuid,
    /// 不可变卖家及装备锁键。
    pub(crate) identity: &'a ListingLockIdentity,
    /// 登记冻结的费率。
    pub(crate) fee_bps: FeeBps,
    /// 登记时固定的服务代次。
    pub(crate) service_epoch: i64,
    /// 登记时已见的拉黑历史事实数。
    pub(crate) blacklist_count_at_register: i64,
}

pub(in crate::database::consignment) async fn insert(
    connection: &mut PgConnection,
    new: NewListing<'_>,
    snapshot: ListingSnapshot,
    now: Timestamp,
) -> Result<(), WriteError> {
    // 不变量 2、3、10：同一顶层事务创建冻结身份与版本，命令和事件由事实写入器补齐。
    sqlx::query(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             created_at, updated_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8, $9, $10, $11, $12, $12, $13)",
    )
    .bind(new.listing_id)
    .bind(new.identity.seller_user_id)
    .bind(new.identity.equipment.world_id())
    .bind(new.identity.equipment.external_equipment_id())
    .bind(snapshot.mode.as_str())
    .bind(snapshot.state.as_str())
    .bind(snapshot.price.value())
    .bind(new.fee_bps.value().cast_signed())
    .bind(snapshot.state_version.value().cast_signed())
    .bind(new.service_epoch)
    .bind(new.blacklist_count_at_register)
    .bind(now)
    .bind(snapshot.expires_at)
    .execute(connection)
    .await?;
    Ok(())
}

pub(in crate::database::consignment) async fn update(
    connection: &mut PgConnection,
    current: &LockedListing,
    snapshot: ListingSnapshot,
    current_snapshot_id: Option<Uuid>,
    now: Timestamp,
) -> Result<(), WriteError> {
    // 不变量 2、3、6：仅更新已经锁定的旧版本，版本连续性及冻结字段由触发器复核。
    let result = sqlx::query(
        "UPDATE public.consignment_listings
         SET state = $3, state_version = $4, price = $5, current_snapshot_id = $6,
             reason_code = $7, updated_at = $8, expires_at = $9
         WHERE id = $1 AND state_version = $2",
    )
    .bind(current.listing_id)
    .bind(current.snapshot.state_version.value().cast_signed())
    .bind(snapshot.state.as_str())
    .bind(snapshot.state_version.value().cast_signed())
    .bind(snapshot.price.value())
    .bind(current_snapshot_id)
    .bind(snapshot.reason.map(ListingReason::as_str))
    .bind(now)
    .bind(snapshot.expires_at)
    .execute(connection)
    .await?;
    if result.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_locked_listing_version"));
    }
    Ok(())
}

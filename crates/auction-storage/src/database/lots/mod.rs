//! Public lot 只读查询适配器。
//!
//! lot 列表和详情必须与当前公开 projection 使用同一数据库快照，并且只返回当前公开
//! 状态允许展示的 preview/live 事实。数据库行、内部策略和装备原始字段在这里映射为
//! 应用层安全投影，HTTP 层不直接接触 SQL 类型。

use std::collections::BTreeMap;

use auction_application::{
    ApplicationError, ApplicationFuture, PublicAuctionProjection, PublicEquipment,
    PublicEquipmentStat, PublicFinalRestriction, PublicLiveLotDetail, PublicLiveLotSummary,
    PublicLotDetail, PublicLotListRequest, PublicLotPage, PublicLotReader, PublicPreviewLotDetail,
    PublicPreviewLotSummary, RepositoryError,
};
use auction_domain::{AuctionRules, BidAmount, EquipmentId};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::PostgresStore;
#[cfg(feature = "sqlite")]
use super::SqliteStore;

const CATEGORY_KEYS: [&str; 8] = ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];

impl PublicLotReader for DatabaseStore {
    fn list_public_lots(
        &self,
        auction_id: Uuid,
        request: PublicLotListRequest,
    ) -> ApplicationFuture<'_, Option<PublicLotPage>> {
        Box::pin(async move {
            validate_lot_request(auction_id, &request)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, auction_id, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, auction_id, request).await,
            }
        })
    }

    fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>> {
        Box::pin(async move {
            if lot_id.is_nil() {
                return Err(storage_error("公开 lot 标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => detail_postgres(store, lot_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => detail_sqlite(store, lot_id).await,
            }
        })
    }
}

/// 为论坛同步快照复用 Public 装备字段校验和 Roll 算法，不建立第二套属性解释规则。
#[cfg(feature = "postgres")]
pub(super) fn map_forum_equipment(
    world_id: &str,
    external_id: &str,
    name: &str,
    category: &str,
    attributes: Value,
) -> Result<PublicEquipment, ApplicationError> {
    map_equipment(world_id, external_id, name, category, attributes, true)
}

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

use mapping::*;
#[cfg(feature = "postgres")]
use postgres::{detail_postgres, list_postgres};
#[cfg(feature = "sqlite")]
use sqlite::{detail_sqlite, list_sqlite};

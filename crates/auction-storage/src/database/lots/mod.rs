//! Public lot 只读查询适配器；列表与详情必须使用同一数据库快照，只返回当前公开状态允许展示的
//! preview/live 事实；数据库行、内部策略和装备字段在这里映射为安全投影，HTTP 层不接触 SQL 类型。

use std::collections::BTreeMap;

use auction_application::{
    ApplicationError, ApplicationFuture, EquipmentColorFacts, EquipmentColorRule, EquipmentGroup,
    PublicAuctionProjection, PublicEquipment, PublicEquipmentStat, PublicFinalRestriction,
    PublicLiveLotDetail, PublicLiveLotSummary, PublicLotDetail, PublicLotListRequest,
    PublicLotPage, PublicLotReader, PublicPreviewLotDetail, PublicPreviewLotSummary,
    equipment_color_decision,
};
use auction_domain::{AuctionRules, BidAmount, EquipmentId};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::PostgresStore;

const CATEGORY_KEYS: [&str; 8] = ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];

/// 装配根注入有效装备标色规则的公开只读适配器，规则在进程启动时冻结。
#[derive(Debug, Clone)]
pub struct DatabasePublicLotReader {
    store: DatabaseStore,
    colors: Vec<EquipmentColorRule>,
}

impl DatabaseStore {
    /// 与论坛渲染器使用同一份已校验规则，统一公开页标签和分页前筛选。
    pub fn public_lot_reader(&self, colors: Vec<EquipmentColorRule>) -> DatabasePublicLotReader {
        DatabasePublicLotReader { store: self.clone(), colors }
    }
}

impl PublicLotReader for DatabasePublicLotReader {
    fn list_public_lots(
        &self,
        auction_id: Uuid,
        request: PublicLotListRequest,
    ) -> ApplicationFuture<'_, Option<PublicLotPage>> {
        Box::pin(async move {
            validate_lot_request(auction_id, &request)?;
            match &self.store {
                DatabaseStore::Postgres(store) => {
                    list_postgres(store, auction_id, request, &self.colors).await
                }
            }
        })
    }

    fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>> {
        Box::pin(async move {
            if lot_id.is_nil() {
                return Err(storage_error("公开 lot 标识无效"));
            }
            match &self.store {
                DatabaseStore::Postgres(store) => {
                    detail_postgres(store, lot_id, &self.colors).await
                }
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
    map_equipment(world_id, external_id, name, category, attributes, true, &[])
}

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;

use mapping::*;
#[cfg(feature = "postgres")]
use postgres::{detail_postgres, list_postgres};

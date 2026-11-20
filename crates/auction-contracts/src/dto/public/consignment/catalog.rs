//! 寄售服务能力、分页、公开目录和可安全显示的装备快照。

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error;
use uuid::Uuid;

use crate::MAX_PUBLIC_PAGE_SIZE;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::states::PublicListingState;
use crate::dto::public::consignment::wire::FEE_BPS_MAX;
use crate::dto::public::consignment::wire::PRICE_MAX;
use crate::dto::public::consignment::wire::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire::equipment_id;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_text;
use crate::dto::public::consignment::wire::serialize_unavailable;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::unavailable;
use crate::dto::public::consignment::wire::uuid;

const MAX_PAGE_OFFSET: i64 = 10_000;

json_object! {
    ()
    /// 匿名可读的寄售能力。前三期收件与购买固定为 false。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ConsignmentStatus {
        pub enabled: bool,
        pub registration_ready: bool,
        pub registration_modes: Vec<ListingMode>,
        pub registration_worlds: Vec<ListingWorld>,
        #[serde(deserialize_with = "unavailable", serialize_with = "serialize_unavailable")]
        pub accepts_mail: bool,
        #[serde(deserialize_with = "unavailable", serialize_with = "serialize_unavailable")]
        pub purchasable: bool,
        #[serde(deserialize_with = "integer::<0, FEE_BPS_MAX, _>")]
        pub fee_bps: i64,
        #[serde(deserialize_with = "integer::<1, 1, _>")]
        pub min_price: i64,
        #[serde(deserialize_with = "integer::<PRICE_MAX, PRICE_MAX, _>")]
        pub max_price: i64,
    } => validate
}

impl ConsignmentStatus {
    fn validate<E: Error>(&self) -> Result<(), E> {
        if self.registration_ready && !self.enabled {
            return Err(E::custom("寄售关闭时不可登记"));
        }
        if !self.registration_ready
            && !(self.registration_modes.is_empty() && self.registration_worlds.is_empty())
        {
            return Err(E::custom("登记未就绪时不得提供可登记模式和世界"));
        }
        Ok(())
    }
}

json_object! {
    ()
    /// Public、本人及 Admin 查询共用的分页响应体，空页仍保留 total。
    #[derive(Debug, Clone, PartialEq)]
    pub struct ListingPage<T> {
        pub items: Vec<T>,
        #[serde(deserialize_with = "integer::<0, SAFE_INTEGER_MAX, _>")]
        pub total: i64,
        #[serde(deserialize_with = "integer::<1, SAFE_INTEGER_MAX, _>")]
        pub page: i64,
        #[serde(deserialize_with = "integer::<1, MAX_PUBLIC_PAGE_SIZE, _>")]
        pub page_size: i64,
    } => validate
}

impl<T> ListingPage<T> {
    fn validate<E: Error>(&self) -> Result<(), E> {
        if (self.page - 1) > MAX_PAGE_OFFSET / self.page_size {
            return Err(E::custom("寄售分页行偏移超出上限"));
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 公开条目白名单。只包含公开身份、装备摘要、标价、卖家显示名和时刻。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ListingSummary {
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        #[serde(deserialize_with = "integer::<1, SAFE_INTEGER_MAX, _>")]
        pub version: i64,
        pub mode: ListingMode,
        pub state: PublicListingState,
        pub world_id: ListingWorld,
        #[serde(deserialize_with = "equipment_id")]
        pub external_equipment_id: String,
        #[serde(deserialize_with = "text::<512, _>")]
        pub name: String,
        #[serde(deserialize_with = "text::<128, _>")]
        pub category: String,
        pub level: EquipmentLevel,
        #[serde(deserialize_with = "integer::<1, PRICE_MAX, _>")]
        pub price: i64,
        pub seller_name: String,
        #[serde(deserialize_with = "timestamp")]
        pub updated_at: Timestamp,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
        #[serde(deserialize_with = "unavailable", serialize_with = "serialize_unavailable")]
        pub purchasable: bool,
    }
}

json_object! {
    ()
    /// 独立的公开详情，由目录摘要与安全快照组成。
    #[derive(Debug, Clone, PartialEq)]
    pub struct ListingDetail {
        pub summary: ListingSummary,
        pub equipment: EquipmentView,
    }
}

json_object! {
    (tag = "kind", rename_all = "snake_case")
    /// 展示用装备等级，与第三方的原始编码分离。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum EquipmentLevel {
        /// 已分配等级。
        Assigned {
            #[serde(deserialize_with = "integer::<1, 100_000, _>")]
            value: i64,
        },
        /// 尚未分配等级。
        Unassigned {},
        /// 灵魂绑定。
        Soulbound {},
    }
}

/// 白名单快照的固定来源标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum EquipmentViewSource {
    /// 第三方装备信息的第一版安全投影。
    #[serde(rename = "hvdata-equip-v1")]
    HvDataEquipV1,
}

json_object! {
    ()
    /// 公开、本人和 Admin 可共用的安全装备快照。
    #[derive(Debug, Clone, PartialEq)]
    pub struct EquipmentView {
        #[serde(deserialize_with = "text::<512, _>")]
        pub name: String,
        #[serde(deserialize_with = "optional_text::<512, _>")]
        pub alt_name: Option<String>,
        #[serde(deserialize_with = "text::<128, _>")]
        pub category: String,
        pub level: EquipmentLevel,
        pub tradeable: bool,
        pub stat_groups: Vec<EquipmentStatGroup>,
        pub source: EquipmentViewSource,
        #[serde(deserialize_with = "timestamp")]
        pub observed_at: Timestamp,
    } => validate
}

impl EquipmentView {
    fn validate<E: Error>(&self) -> Result<(), E> {
        if matches!(self.level, EquipmentLevel::Soulbound {}) && self.tradeable {
            return Err(E::custom("灵魂绑定装备不可交易"));
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 装备属性的命名分组。
    #[derive(Debug, Clone, PartialEq)]
    pub struct EquipmentStatGroup {
        #[serde(deserialize_with = "text::<128, _>")]
        pub name: String,
        pub stats: Vec<EquipmentStat>,
    }
}

json_object! {
    ()
    /// 允许显示的单条装备属性。
    #[derive(Debug, Clone, PartialEq)]
    pub struct EquipmentStat {
        #[serde(deserialize_with = "text::<128, _>")]
        pub name: String,
        #[serde(deserialize_with = "stat_number")]
        pub value: f64,
        #[serde(deserialize_with = "optional_stat_number")]
        pub base: Option<f64>,
    }
}

fn stat_number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    checked_stat_number(f64::deserialize(deserializer)?)
}

fn optional_stat_number<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<f64>, D::Error> {
    Option::<f64>::deserialize(deserializer)?.map(checked_stat_number).transpose()
}

fn checked_stat_number<E: Error>(value: f64) -> Result<f64, E> {
    if (-1e9..=1e9).contains(&value) {
        return Ok(value);
    }
    Err(E::custom("装备属性数值超出展示范围"))
}

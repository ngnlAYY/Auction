//! Public lot 目录、装备投影和 projection 读取端口。

use std::collections::BTreeMap;

use auction_domain::EquipmentId;
use auction_domain::valid_text as public_text;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::{ApplicationFuture, EquipmentGroup};

/// 公开 lot 读取允许的最大 legacy offset，防止深分页拖垮数据库。
pub const MAX_PUBLIC_LOT_OFFSET: i64 = 10_000;

/// 公开 lot 分类对应的稳定 JSON key。
pub const PUBLIC_LOT_CATEGORIES: [&str; 8] =
    ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLotListRequest {
    pub category: Option<String>,
    pub equipment_group: Option<EquipmentGroup>,
    /// 数字等级的包含式上限；None 不限制数字等级，不把无等级装备视为零级。
    pub max_level: Option<i64>,
    /// 无数字等级装备独立控制；未指定上限时也生效。
    pub include_unleveled: bool,
    pub limit: i64,
    pub offset: i64,
}

impl PublicLotListRequest {
    /// 在应用端口边界拒绝未定义分类、非正页大小和深 offset；数量不设业务上限。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1 || self.offset < 0 {
            return Err("公开 lot 分页参数无效");
        }
        if self.offset > MAX_PUBLIC_LOT_OFFSET {
            return Err("公开 lot 分页 offset 超出上限");
        }
        if self
            .category
            .as_deref()
            .is_some_and(|category| !PUBLIC_LOT_CATEGORIES[1..].contains(&category))
        {
            return Err("公开 lot 分类参数无效");
        }
        if self.max_level.is_some_and(|level| level < 0) {
            return Err("公开 lot 等级上限无效");
        }
        Ok(())
    }

    /// 目录筛选不改变装备事实或出价资格；无等级是否保留由独立选项决定。
    pub fn matches_level(&self, level: Option<i64>) -> bool {
        match level {
            Some(level) => level >= 0 && self.max_level.is_none_or(|maximum| level <= maximum),
            None => self.include_unleveled,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublicAuctionProjection {
    pub auction_id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: DateTime<Utc>,
    pub scheduled_ends_at: Option<DateTime<Utc>>,
    pub preview_revision: i64,
    pub final_restriction: Option<PublicFinalRestriction>,
}

impl PublicAuctionProjection {
    /// 校验公开投影的持久字段、展示分支和竞拍能力必须互相一致。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || !public_text(&self.world_id, 64)
            || self.sequence <= 0
            || is_go_zero_time(self.opens_at)
            || self.preview_revision < 0
        {
            return Err("公开 lot projection 无效");
        }
        match self.auction_status.as_str() {
            "preview" => {
                if self.projection_kind != "preview"
                    || !matches!(self.presentation_mode.as_str(), "preview" | "opening")
                    || self.bidding_open
                    || self.scheduled_ends_at.is_some()
                    || self.final_restriction.is_some()
                {
                    return Err("公开预览 lot projection 无效");
                }
            }
            "live" => {
                if self.projection_kind != "live"
                    || self.presentation_mode != "live"
                    || !self.bidding_open
                    || self.final_restriction.is_some()
                {
                    return Err("公开正式 lot projection 无效");
                }
            }
            "final" => {
                let Some(restriction) = self.final_restriction else {
                    return Err("公开终阶段 lot projection 缺少限制");
                };
                if self.projection_kind != "live"
                    || self.presentation_mode != "live"
                    || !self.bidding_open
                    || restriction.new_slow_proxy_allowed
                    || !restriction.existing_slow_proxy_converted
                {
                    return Err("公开终阶段 lot projection 无效");
                }
            }
            "settling" | "settled" => {
                if self.projection_kind != "live"
                    || self.presentation_mode != "live"
                    || self.bidding_open
                    || self.final_restriction.is_some()
                {
                    return Err("公开收口 lot projection 无效");
                }
            }
            _ => return Err("公开 lot projection 状态无效"),
        }
        if self.scheduled_ends_at.is_some_and(|value| value < self.opens_at) {
            return Err("公开 lot projection 时间关系无效");
        }
        Ok(())
    }
}

/// 终阶段对代理操作的公开限制说明。
#[derive(Debug, Clone, Copy)]
pub struct PublicFinalRestriction {
    pub new_slow_proxy_allowed: bool,
    pub existing_slow_proxy_converted: bool,
}

/// 公开装备 stat 的渲染字段。
#[derive(Debug, Clone)]
pub struct PublicEquipmentStat {
    pub value: String,
    pub base: Option<f64>,
    pub roll: Option<f64>,
}

/// 不包含 key、邮件和托管信息的公开装备投影。
#[derive(Debug, Clone)]
pub struct PublicEquipment {
    pub name: String,
    pub category: String,
    pub equipment_group: EquipmentGroup,
    pub level: Option<i64>,
    pub info_summary: String,
    pub stats: BTreeMap<String, PublicEquipmentStat>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
    pub attributes: Value,
}

impl PublicEquipment {
    /// 按 HV Primary Attributes 的固定顺序生成论坛与 Web 共用的属性首字母。
    pub fn primary_attribute_initials(&self) -> Option<String> {
        const PRIMARY_ATTRIBUTES: [(&str, char); 6] = [
            ("Strength", 'S'),
            ("Dexterity", 'D'),
            ("Agility", 'A'),
            ("Endurance", 'E'),
            ("Intelligence", 'I'),
            ("Wisdom", 'W'),
        ];

        let mut initials = String::with_capacity(PRIMARY_ATTRIBUTES.len());
        for (attribute_name, initial) in PRIMARY_ATTRIBUTES {
            if self.stats.keys().any(|stat_name| stat_name.eq_ignore_ascii_case(attribute_name)) {
                initials.push(initial);
            }
        }
        (!initials.is_empty()).then_some(initials)
    }

    /// 校验公开装备投影；preview 的空分类由调用方按 preview 语义单独允许。
    pub fn validate(&self, category_required: bool) -> Result<(), &'static str> {
        if !public_text(&self.name, 512)
            || category_required && !public_text(&self.category, 128)
            || !self.category.is_empty() && !public_text(&self.category, 128)
            || self.level.is_some_and(|level| level < 0)
            || !self.info_summary.is_empty() && !public_text(&self.info_summary, 512)
            || !valid_game_equipment_url(&self.game_equipment_url)
            || self.avg_roll.is_some_and(|value| !value.is_finite())
        {
            return Err("公开装备投影无效");
        }
        for (name, stat) in &self.stats {
            if !public_text(name, 128)
                || !public_text(&stat.value, 256)
                || stat.base.is_some_and(|value| !value.is_finite())
                || stat.roll.is_some_and(|value| !value.is_finite())
            {
                return Err("公开装备 stat 无效");
            }
        }
        Ok(())
    }
}

/// 正式竞拍 lot 的公开摘要。
#[derive(Debug, Clone)]
pub struct PublicLiveLotSummary {
    pub id: Uuid,
    /// 只供同进程完整 URL 解析器绑定密文 owner；HTTP DTO 映射不会公开该 UUID。
    pub equipment_id: EquipmentId,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    pub leader_name: Option<String>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub state_version: i64,
    pub equipment: PublicEquipment,
}

impl PublicLiveLotSummary {
    /// 校验正式 lot 的价格、领先者、状态和结束时间关系。
    pub fn validate(
        &self,
        auction_status: &str,
        opens_at: DateTime<Utc>,
    ) -> Result<(), &'static str> {
        if self.id.is_nil()
            || self.equipment_id.is_nil()
            || self.auction_id.is_nil()
            || self.lot_number <= 0
            || !matches!(self.status.as_str(), "live" | "sold" | "unsold")
            || !public_text(&self.seller_name, 256)
            || self.start_price <= 0
            || self.current_price <= 0
            || self.current_price < self.start_price
            || self.next_bid <= 0
            || self.state_version <= 0
            || self.has_leader != self.leader_name.is_some()
            || self.leader_name.as_deref().is_some_and(|value| !public_text(value, 256))
            || self.effective_end_at.is_some_and(|value| value < opens_at)
            || self.status != "unsold" && self.effective_end_at.is_none()
            || self.status == "sold" && !self.has_leader
            || self.status == "unsold" && self.has_leader
            || auction_status == "settled" && self.status == "live"
        {
            return Err("公开正式 lot 无效");
        }
        if self.has_leader {
            if self.next_bid <= self.current_price {
                return Err("公开正式 lot 下一口金额无效");
            }
        } else if self.current_price != self.start_price || self.next_bid != self.start_price {
            return Err("公开正式 lot 无领先者价格无效");
        }
        self.equipment.validate(true)
    }
}

/// 正式竞拍 lot 详情；attributes 只在详情分支通过 HTTP 映射公开。
#[derive(Debug, Clone)]
pub struct PublicLiveLotDetail {
    pub summary: PublicLiveLotSummary,
}

impl PublicLiveLotDetail {
    /// 校验正式详情的装备属性是可公开的 JSON 对象。
    pub fn validate(
        &self,
        auction_status: &str,
        opens_at: DateTime<Utc>,
    ) -> Result<(), &'static str> {
        self.summary.validate(auction_status, opens_at)?;
        if !self.summary.equipment.attributes.is_object() {
            return Err("公开正式 lot 详情装备属性无效");
        }
        Ok(())
    }
}

/// 预览 lot 的公开摘要。
#[derive(Debug, Clone)]
pub struct PublicPreviewLotSummary {
    pub id: Uuid,
    /// 只供同进程完整 URL 解析器绑定密文 owner；HTTP DTO 映射不会公开该 UUID。
    pub equipment_id: EquipmentId,
    pub auction_id: Uuid,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_group: EquipmentGroup,
    pub equipment_level: Option<i64>,
    pub primary_attribute_initials: Option<String>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
}

impl PublicPreviewLotSummary {
    /// 校验预览 lot 的完整摘要事实；分类允许为空以保持旧 wire 语义。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.is_nil()
            || self.equipment_id.is_nil()
            || self.auction_id.is_nil()
            || !public_text(&self.code, 64)
            || self.status != "pending"
            || !public_text(&self.seller_name, 256)
            || self.start_price <= 0
            || !public_text(&self.equipment_name, 512)
            || !self.equipment_category.is_empty() && !public_text(&self.equipment_category, 128)
            || self.equipment_level.is_some_and(|level| level < 0)
            || self
                .primary_attribute_initials
                .as_deref()
                .is_some_and(|value| !valid_primary_attribute_initials(value))
            || self.avg_roll.is_some_and(|value| !value.is_finite())
            || !valid_game_equipment_url(&self.game_equipment_url)
        {
            return Err("公开预览 lot 无效");
        }
        Ok(())
    }
}

/// 预览 lot 详情。
#[derive(Debug, Clone)]
pub struct PublicPreviewLotDetail {
    pub summary: PublicPreviewLotSummary,
    pub equipment: PublicEquipment,
}

impl PublicPreviewLotDetail {
    /// 校验预览摘要和详情装备的名称、分类、等级及链接逐字段对应。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.summary.validate()?;
        self.equipment.validate(false)?;
        if self.equipment.name != self.summary.equipment_name
            || self.equipment.category != self.summary.equipment_category
            || self.equipment.equipment_group != self.summary.equipment_group
            || self.equipment.level != self.summary.equipment_level
            || self.equipment.primary_attribute_initials()
                != self.summary.primary_attribute_initials
            || self.equipment.avg_roll != self.summary.avg_roll
            || self.equipment.game_equipment_url != self.summary.game_equipment_url
        {
            return Err("公开预览 lot 详情装备与摘要不一致");
        }
        Ok(())
    }
}

/// 当前公开 projection 下的单个 lot 详情。
#[derive(Debug, Clone)]
pub struct PublicLotDetail {
    /// 只供同进程可信适配器绑定加密 key；HTTP DTO 映射不会公开该内部 UUID。
    pub equipment_id: EquipmentId,
    pub projection: PublicAuctionProjection,
    pub preview: Option<PublicPreviewLotDetail>,
    pub live: Option<PublicLiveLotDetail>,
}

impl PublicLotDetail {
    /// 校验详情的 projection 分支、lot 归属和 preview/live 互斥性。
    pub fn validate(&self, lot_id: Uuid) -> Result<(), &'static str> {
        if lot_id.is_nil() || self.equipment_id.is_nil() || self.projection.validate().is_err() {
            return Err("公开 lot 详情 projection 无效");
        }
        if self.projection.projection_kind == "preview" {
            let Some(preview) = &self.preview else {
                return Err("公开预览 lot 详情缺失");
            };
            if self.live.is_some()
                || preview.summary.id != lot_id
                || preview.summary.equipment_id != self.equipment_id
                || preview.summary.auction_id != self.projection.auction_id
            {
                return Err("公开预览 lot 详情归属无效");
            }
            return preview.validate();
        }
        let Some(live) = &self.live else {
            return Err("公开正式 lot 详情缺失");
        };
        if self.preview.is_some()
            || live.summary.id != lot_id
            || live.summary.equipment_id != self.equipment_id
            || live.summary.auction_id != self.projection.auction_id
        {
            return Err("公开正式 lot 详情归属无效");
        }
        live.validate(&self.projection.auction_status, self.projection.opens_at)
    }
}

#[derive(Debug, Clone)]
pub struct PublicLotPage {
    pub projection: PublicAuctionProjection,
    pub preview: Vec<PublicPreviewLotSummary>,
    pub live: Vec<PublicLiveLotSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub category_counts: BTreeMap<String, i64>,
}

impl PublicLotPage {
    /// 分组及等级筛选必须先于统计和分页；返回页的两种投影复核同一请求条件。
    pub fn validate_filters(&self, request: &PublicLotListRequest) -> Result<(), &'static str> {
        if self.preview.iter().any(|lot| {
            request.equipment_group.is_some_and(|group| lot.equipment_group != group)
                || !request.matches_level(lot.equipment_level)
        }) || self.live.iter().any(|lot| {
            request.equipment_group.is_some_and(|group| lot.equipment.equipment_group != group)
                || !request.matches_level(lot.equipment.level)
        }) {
            return Err("公开 lot 页装备筛选无效");
        }
        Ok(())
    }

    /// 校验列表的完整投影、分页、分类统计和筛选归属。
    pub fn validate(&self, auction_id: Uuid, category: Option<&str>) -> Result<(), &'static str> {
        if auction_id.is_nil()
            || self.projection.validate().is_err()
            || self.projection.auction_id != auction_id
            || self.limit < 1
            || self.offset < 0
            || self.offset > MAX_PUBLIC_LOT_OFFSET
            || self.total < 0
            || self.preview.len() > self.limit as usize
            || self.live.len() > self.limit as usize
        {
            return Err("公开 lot 页无效");
        }
        if !matches!(category, None | Some("one" | "two" | "sta" | "shd" | "clo" | "lig" | "hea")) {
            return Err("公开 lot 页分类无效");
        }
        if self.category_counts.len() != PUBLIC_LOT_CATEGORIES.len()
            || PUBLIC_LOT_CATEGORIES
                .iter()
                .any(|key| self.category_counts.get(*key).is_none_or(|value| *value < 0))
        {
            return Err("公开 lot 页分类统计无效");
        }
        let counts = &self.category_counts;
        let Some(known) = PUBLIC_LOT_CATEGORIES[1..]
            .iter()
            .try_fold(0_i64, |total, key| total.checked_add(counts[*key]))
        else {
            return Err("公开 lot 页分类统计溢出");
        };
        if known > counts["all"] || self.total > counts["all"] {
            return Err("公开 lot 页分类统计不一致");
        }
        let expected_total = category.map_or(counts["all"], |key| counts[key]);
        if self.total != expected_total {
            return Err("公开 lot 页总数与分类统计不一致");
        }
        let expected_category = category.map(category_value);
        if self.projection.projection_kind == "preview" {
            if !self.live.is_empty() {
                return Err("公开预览 lot 页包含正式分支");
            }
            for lot in &self.preview {
                if lot.auction_id != auction_id
                    || expected_category.is_some_and(|value| lot.equipment_category != value)
                {
                    return Err("公开预览 lot 页归属或分类无效");
                }
                lot.validate()?;
            }
        } else {
            if !self.preview.is_empty() {
                return Err("公开正式 lot 页包含预览分支");
            }
            for lot in &self.live {
                if lot.auction_id != auction_id
                    || expected_category.is_some_and(|value| lot.equipment.category != value)
                {
                    return Err("公开正式 lot 页归属或分类无效");
                }
                lot.validate(&self.projection.auction_status, self.projection.opens_at)?;
            }
        }
        Ok(())
    }
}

fn category_value(value: &str) -> &'static str {
    match value {
        "one" => "One-Handed",
        "two" => "Two-Handed",
        "sta" => "Staff",
        "shd" => "Shield",
        "clo" => "Cloth",
        "lig" => "Light",
        "hea" => "Heavy",
        _ => "",
    }
}

fn valid_primary_attribute_initials(value: &str) -> bool {
    let mut cursor = 0;
    let ordered = b"SDAEIW";
    !value.is_empty()
        && value.bytes().all(|byte| {
            let Some(position) = ordered[cursor..].iter().position(|candidate| *candidate == byte)
            else {
                return false;
            };
            cursor += position + 1;
            true
        })
}

fn is_go_zero_time(value: DateTime<Utc>) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}

fn valid_game_equipment_url(value: &str) -> bool {
    (value.starts_with("https://hentaiverse.org/equip/")
        || value.starts_with("https://hentaiverse.org/isekai/equip/"))
        && value.rsplit('/').next().is_some_and(|tail| !tail.is_empty())
        && !value.contains(['?', '#', '@'])
        && !value.chars().any(char::is_control)
}

/// Public lot 只读端口；未出现在当前公开 projection 的 lot 必须返回 None。
pub trait PublicLotReader: Send + Sync {
    fn list_public_lots(
        &self,
        auction_id: Uuid,
        request: PublicLotListRequest,
    ) -> ApplicationFuture<'_, Option<PublicLotPage>>;

    fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>>;
}

//! Public API 查询参数和目录展示模型。

use super::{LotEnvelope, LotItems};

/// 公开价格历史的旧版筛选与分页参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriceHistoryQuery {
    pub world: Option<String>,
    pub name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_price: Option<String>,
    pub max_price: Option<String>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
    pub sort: String,
    pub page: i64,
}

impl Default for PriceHistoryQuery {
    fn default() -> Self {
        Self {
            world: None,
            name: None,
            from: None,
            to: None,
            min_price: None,
            max_price: None,
            buyer: None,
            seller: None,
            sort: "closed_desc".to_owned(),
            page: 1,
        }
    }
}

/// 目录等级查询的浏览器投影；保留整数精度，不把无等级等同于零级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CatalogLevelFilter {
    pub max_level: Option<i64>,
    pub include_unleveled: bool,
}

impl Default for CatalogLevelFilter {
    fn default() -> Self {
        Self { max_level: None, include_unleveled: true }
    }
}

impl CatalogLevelFilter {
    /// 草稿与 URL 共用严格解析；留空仅表示不限，非法值不得悄悄扩大查询。
    pub fn parse(max_level: &str, include_unleveled: &str) -> Result<Self, ()> {
        let max_level = if max_level.is_empty() {
            None
        } else {
            if !max_level.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(());
            }
            Some(max_level.parse::<i64>().map_err(|_| ())?)
        };
        let include_unleveled = match include_unleveled {
            "true" => true,
            "false" => false,
            _ => return Err(()),
        };
        Ok(Self { max_level, include_unleveled })
    }

    /// 只用于校验响应是否属于本次查询，不在浏览器分页后过滤或推算总数。
    pub fn matches_level(self, level: Option<i64>) -> bool {
        match level {
            Some(level) => level >= 0 && self.max_level.is_none_or(|maximum| level <= maximum),
            None => self.include_unleveled,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogRow {
    pub lot_id: String,
    pub code: String,
    pub name: String,
    pub game_equipment_url: String,
    pub category: String,
    pub equipment_group: String,
    pub status: String,
    pub seller_name: String,
    pub level: Option<i64>,
    pub primary_attribute_initials: Option<String>,
    pub avg_roll: Option<f64>,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub leader_name: Option<String>,
    pub remaining: String,
}

/// 将稳定 lot wire 契约转换为页面表格所需的展示行。
pub(crate) trait LotEnvelopeRows {
    fn rows(&self) -> Vec<CatalogRow>;
}

impl LotEnvelopeRows for LotEnvelope {
    fn rows(&self) -> Vec<CatalogRow> {
        match self.lots.as_ref() {
            Some(LotItems::Preview(lots)) => lots
                .iter()
                .map(|lot| CatalogRow {
                    lot_id: lot.id.clone(),
                    code: lot.code.clone(),
                    name: lot.equipment_name.clone(),
                    game_equipment_url: lot.game_equipment_url.clone(),
                    // 缺失分类沿用旧版归入 Other，但不把缺失重新编码成 wire 空字符串。
                    category: lot.equipment_category.clone().unwrap_or_else(|| "other".to_owned()),
                    equipment_group: lot.equipment_group.clone(),
                    status: lot.status.clone(),
                    seller_name: lot.seller_name.clone(),
                    level: lot.equipment_level,
                    primary_attribute_initials: lot.primary_attribute_initials.clone(),
                    avg_roll: lot.avg_roll,
                    start_price: lot.start_price,
                    current_price: lot.start_price,
                    next_bid: lot.start_price,
                    leader_name: None,
                    remaining: String::new(),
                })
                .collect(),
            Some(LotItems::Live(lots)) => lots
                .iter()
                .map(|lot| CatalogRow {
                    lot_id: lot.id.clone(),
                    code: lot_code_for(&lot.equipment.category, lot.lot_number),
                    name: lot.equipment.name.clone(),
                    game_equipment_url: lot.equipment.game_equipment_url.clone(),
                    category: lot.equipment.category.clone(),
                    equipment_group: lot.equipment.equipment_group.clone(),
                    status: lot.status.clone(),
                    seller_name: lot.seller_name.clone(),
                    level: lot.equipment.level,
                    primary_attribute_initials: lot.equipment.primary_attribute_initials.clone(),
                    avg_roll: lot.equipment.avg_roll,
                    start_price: lot.start_price,
                    current_price: lot.current_price,
                    next_bid: lot.next_bid,
                    leader_name: lot.leader_name.clone(),
                    remaining: lot.effective_end_at.clone().unwrap_or_default(),
                })
                .collect(),
            None => Vec::new(),
        }
    }
}

pub(crate) fn lot_code_for(category: &str, lot_number: i64) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
    format!("{prefix}{lot_number:02}")
}

//! Public API 查询参数和目录展示模型。

use super::{LotEnvelope, LotItems};

/// 公开价格历史的旧版筛选与分页参数。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PriceHistoryQuery {
    pub world: Option<String>,
    pub name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_price: Option<String>,
    pub max_price: Option<String>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
    pub page: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogRow {
    pub lot_id: String,
    pub code: String,
    pub name: String,
    pub category: String,
    pub status: String,
    pub seller_name: String,
    pub level: Option<i64>,
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
                    category: lot.equipment_category.clone(),
                    status: lot.status.clone(),
                    seller_name: lot.seller_name.clone(),
                    level: lot.equipment_level,
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
                    category: lot.equipment.category.clone(),
                    status: lot.status.clone(),
                    seller_name: lot.seller_name.clone(),
                    level: lot.equipment.level,
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

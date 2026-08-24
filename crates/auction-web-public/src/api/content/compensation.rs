//! Public 补偿池 HTTP DTO 与请求。

use serde::Deserialize;

use super::{ApiResponse, send_get};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct CompensationRound {
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct CompensationMovement {
    pub sequence: i32,
    pub lot_id: Option<String>,
    pub lot_number: Option<i32>,
    pub equipment_name: Option<String>,
    pub equipment_category: Option<String>,
    pub kind: String,
    pub amount: i64,
    pub balance_after: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct CompensationPoolPage {
    pub balance: i64,
    pub updated_at: Option<String>,
    pub rounds: Vec<CompensationRound>,
    pub page: Option<i64>,
    pub page_size: i64,
    pub total: Option<i64>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct CompensationMovementPage {
    pub round: CompensationRound,
    pub movements: Vec<CompensationMovement>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

pub(crate) async fn fetch_compensation_pool(page: i64) -> Result<CompensationPoolPage, String> {
    let page = page.max(1);
    let url = format!("/api/v1/compensation-pool?page={page}&page_size=25");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<CompensationPoolPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_compensation_movements(
    auction_id: String,
    page: i64,
) -> Result<CompensationMovementPage, String> {
    let page = page.max(1);
    let url = format!("/api/v1/compensation-pool/{auction_id}/movements?page={page}&page_size=50");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<CompensationMovementPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

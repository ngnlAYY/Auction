//! Public 补偿池 HTTP DTO 与请求。

pub(crate) use auction_contracts::{
    PublicCompensationMovementPageResponse as CompensationMovementPage,
    PublicCompensationMovementResponse as CompensationMovement,
    PublicCompensationResponse as CompensationPoolPage,
    PublicCompensationRoundResponse as CompensationRound,
};

use super::{ApiResponse, send_get};

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

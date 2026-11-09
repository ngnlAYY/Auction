//! Public 补偿池 HTTP DTO 与请求。

pub(crate) use auction_contracts::{
    PublicCompensationLotPageResponse as CompensationLotPage,
    PublicCompensationLotResponse as CompensationLot,
    PublicCompensationResponse as CompensationPoolPage,
    PublicCompensationRoundResponse as CompensationRound,
};

use super::{decoder, send_get};

pub(crate) async fn fetch_compensation_pool(
    page: i64,
    page_size: i64,
) -> Result<CompensationPoolPage, String> {
    let page = page.max(1);
    let url = format!("/api/v1/compensation-pool?page={page}&page_size={page_size}");
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_compensation_pool(response).await
}

pub(crate) async fn fetch_compensation_lots(
    auction_id: String,
    page: i64,
    page_size: i64,
) -> Result<CompensationLotPage, String> {
    let page = page.max(1);
    let url =
        format!("/api/v1/compensation-pool/{auction_id}/lots?page={page}&page_size={page_size}");
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_compensation_lots(response).await
}

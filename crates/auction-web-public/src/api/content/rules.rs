//! Public 规则文档 HTTP DTO 与请求。

pub(crate) use auction_contracts::PublicRulesResponse as RulesDocument;

use super::{ApiResponse, send_get};

pub(crate) async fn fetch_rules() -> Result<RulesDocument, String> {
    let response = send_get("/api/v1/rules").await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<RulesDocument> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

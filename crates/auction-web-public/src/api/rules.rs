//! Public 规则文档 HTTP DTO 与请求。

use serde::Deserialize;

use super::{ApiResponse, send_get};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RulesDocument {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub revision: i64,
    pub published_at: String,
}

pub(crate) async fn fetch_rules() -> Result<RulesDocument, String> {
    let response = send_get("/api/v1/rules").await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<RulesDocument> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

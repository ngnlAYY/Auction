//! Public 规则文档 HTTP DTO 与请求。

pub(crate) use auction_contracts::PublicRulesResponse as RulesDocument;

use super::{decoder, send_get};
use url::form_urlencoded;

pub(crate) async fn fetch_rules(locale: &str) -> Result<RulesDocument, String> {
    let query =
        form_urlencoded::Serializer::new(String::new()).append_pair("lang", locale).finish();
    let response = send_get(&format!("/api/v1/rules?{query}"))
        .await
        .map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_rules(response).await
}

//! Public 站点文档（规则、关于）HTTP DTO 与请求。两种文档的响应形状相同，只有路径不同。

pub(crate) use auction_contracts::PublicRulesResponse as RulesDocument;

use url::form_urlencoded;

use super::{decoder, send_get};

/// 公开端可阅读的站点文档种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SiteDocument {
    Rules,
    About,
}

impl SiteDocument {
    fn api_path(self) -> &'static str {
        match self {
            Self::Rules => "/api/v1/rules",
            Self::About => "/api/v1/about",
        }
    }
}

/// “关于”尚未发布时服务端返回的确定错误码；页面把它展示为空状态而不是故障。
pub(crate) const DOCUMENT_UNPUBLISHED: &str = "about_unpublished";

pub(crate) async fn fetch_site_document(
    kind: SiteDocument,
    locale: &str,
) -> Result<RulesDocument, String> {
    let query =
        form_urlencoded::Serializer::new(String::new()).append_pair("lang", locale).finish();
    let response = send_get(&format!("{}?{query}", kind.api_path()))
        .await
        .map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_rules(response).await
}

//! forum 相关 DTO。
// 管理员输入的预览正文和派生标识可能含装备 key；Debug 仅保留安全形状信息。

use super::*;

/// Admin 论坛预览请求；HTML 与纯文本必须恰好提供一个。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminForumPreviewRequest {
    pub html: Option<String>,
    pub text: Option<String>,
}

impl std::fmt::Debug for AdminForumPreviewRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumPreviewRequest")
            .field("html_bytes", &self.html.as_ref().map(String::len))
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .finish()
    }
}

/// Admin 论坛预览中的 lot。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumLotResponse {
    pub code: String,
    pub category: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seller: Option<String>,
    pub start_price: i64,
    pub material: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_bidder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<i64>,
}

impl std::fmt::Debug for AdminForumLotResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumLotResponse")
            .field("code", &"[REDACTED]")
            .field("category", &"[REDACTED]")
            .field("description_bytes", &self.description.len())
            .field("seller", &self.seller.as_ref().map(|_| "[REDACTED]"))
            .field("start_price", &self.start_price)
            .field("material", &self.material)
            .field("current_bidder", &self.current_bidder.as_ref().map(|_| "[REDACTED]"))
            .field("current_price", &self.current_price)
            .finish()
    }
}

/// Admin 论坛预览目录。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumCatalogResponse {
    pub categories: Vec<String>,
    pub lots: Vec<AdminForumLotResponse>,
    pub warnings: Vec<AdminForumWarningResponse>,
}

impl std::fmt::Debug for AdminForumCatalogResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumCatalogResponse")
            .field("category_count", &self.categories.len())
            .field("lots", &self.lots)
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// Admin 论坛预览帖子。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumPostResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub text: String,
    pub edited: bool,
}

impl std::fmt::Debug for AdminForumPostResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumPostResponse")
            .field("id", &"[REDACTED]")
            .field("author_id", &self.author_id.as_ref().map(|_| "[REDACTED]"))
            .field("author", &self.author.as_ref().map(|_| "[REDACTED]"))
            .field("text_bytes", &self.text.len())
            .field("edited", &self.edited)
            .finish()
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumCommandResponse {
    pub line: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub lot_codes: Vec<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proxy_lower: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
}

impl std::fmt::Debug for AdminForumCommandResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumCommandResponse")
            .field("line", &self.line)
            .field("text_bytes", &self.text.len())
            .field("actor", &self.actor.as_ref().map(|_| "[REDACTED]"))
            .field("lot_code_count", &self.lot_codes.len())
            .field("kind", &"[REDACTED]")
            .field("amount", &self.amount)
            .field("proxy", &self.proxy.as_ref().map(|_| "[REDACTED]"))
            .field("proxy_lower", &self.proxy_lower)
            .field("notify", &self.notify.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Admin 论坛预览帖子命令集合。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumBidPostResponse {
    pub post: AdminForumPostResponse,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<AdminForumCommandResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AdminForumWarningResponse>,
    pub ignored: bool,
}

/// Admin 论坛预览警告。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumWarningResponse {
    pub line: usize,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_id: Option<String>,
}

impl std::fmt::Debug for AdminForumWarningResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminForumWarningResponse")
            .field("line", &self.line)
            .field("code", &"[REDACTED]")
            .field("message_bytes", &self.message.len())
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .field("post_id", &self.post_id.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Admin 论坛拍卖预览成功响应数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminForumPreviewResponse {
    pub catalog: AdminForumCatalogResponse,
    pub posts: Vec<AdminForumBidPostResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AdminForumWarningResponse>,
}

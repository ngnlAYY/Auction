//! Admin 论坛拍卖预览端口。
//! 预览只解析管理员提交的 HTML 或文本快照，不写数据库、不调用论坛网关，也不把预览结果
//! 当作已经发布的论坛事实。解析器由 `auction-integrations` 提供，HTTP 只负责契约映射。

use std::fmt;

use auction_domain::valid_text;
use thiserror::Error;

use super::ApplicationFuture;

/// 旧 Admin 论坛预览请求体的最大字节数。
pub const MAX_ADMIN_FORUM_PREVIEW_BYTES: usize = 2 << 20;

/// Admin 论坛预览输入；HTML 与纯文本必须恰好提供一个。
pub struct AdminForumPreviewCommand {
    pub html: Option<String>,
    pub text: Option<String>,
}

impl fmt::Debug for AdminForumPreviewCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumPreviewCommand")
            .field("html_bytes", &self.html.as_ref().map(String::len))
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .finish()
    }
}

impl AdminForumPreviewCommand {
    /// 校验输入选择和边界，不把原始内容写入错误或日志。
    pub fn validate(&self) -> Result<(), AdminForumPreviewError> {
        let html = self.html.as_deref().filter(|value| !value.trim().is_empty());
        let text = self.text.as_deref().filter(|value| !value.trim().is_empty());
        if html.is_some() == text.is_some()
            || html.is_some_and(|value| value.len() > MAX_ADMIN_FORUM_PREVIEW_BYTES)
            || text.is_some_and(|value| value.len() > MAX_ADMIN_FORUM_PREVIEW_BYTES)
        {
            return Err(AdminForumPreviewError::InvalidInput);
        }
        Ok(())
    }
}

/// 解析后的论坛拍品行。
#[derive(Clone)]
pub struct AdminForumLot {
    pub code: String,
    pub category: String,
    pub description: String,
    pub seller: Option<String>,
    pub start_price: i64,
    pub material: bool,
    pub current_bidder: Option<String>,
    pub current_price: Option<i64>,
}

impl fmt::Debug for AdminForumLot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumLot")
            .field("description_bytes", &self.description.len())
            .field("seller_present", &self.seller.is_some())
            .field("start_price", &self.start_price)
            .field("material", &self.material)
            .field("current_bidder_present", &self.current_bidder.is_some())
            .field("current_price", &self.current_price)
            .finish()
    }
}

/// 解析后的论坛目录。
#[derive(Clone)]
pub struct AdminForumCatalog {
    pub categories: Vec<String>,
    pub lots: Vec<AdminForumLot>,
    pub warnings: Vec<AdminForumWarning>,
}

impl fmt::Debug for AdminForumCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumCatalog")
            .field("category_count", &self.categories.len())
            .field("lot_count", &self.lots.len())
            .field("warning_count", &self.warnings.len())
            .finish()
    }
}

/// 解析后的论坛帖子。
#[derive(Clone)]
pub struct AdminForumPost {
    pub id: String,
    pub author_id: Option<String>,
    pub author: Option<String>,
    pub text: String,
    pub edited: bool,
}

impl fmt::Debug for AdminForumPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumPost")
            .field("author_id_present", &self.author_id.is_some())
            .field("author_present", &self.author.is_some())
            .field("text_bytes", &self.text.len())
            .field("edited", &self.edited)
            .finish()
    }
}

#[derive(Clone)]
pub struct AdminForumCommand {
    pub line: usize,
    pub text: String,
    pub actor: Option<String>,
    pub lot_codes: Vec<String>,
    pub kind: String,
    pub amount: Option<i64>,
    pub proxy: Option<String>,
    pub proxy_lower: bool,
    pub notify: Option<String>,
}

impl fmt::Debug for AdminForumCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumCommand")
            .field("line", &self.line)
            .field("text_bytes", &self.text.len())
            .field("actor_present", &self.actor.is_some())
            .field("lot_count", &self.lot_codes.len())
            .field("amount", &self.amount)
            .field("proxy_present", &self.proxy.is_some())
            .field("proxy_lower", &self.proxy_lower)
            .field("notify_present", &self.notify.is_some())
            .finish()
    }
}

/// 解析后的帖子命令集合。
#[derive(Clone)]
pub struct AdminForumBidPost {
    pub post: AdminForumPost,
    pub commands: Vec<AdminForumCommand>,
    pub warnings: Vec<AdminForumWarning>,
    pub ignored: bool,
}

impl fmt::Debug for AdminForumBidPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumBidPost")
            .field("post", &self.post)
            .field("command_count", &self.commands.len())
            .field("warning_count", &self.warnings.len())
            .field("ignored", &self.ignored)
            .finish()
    }
}

/// 解析警告；文本只保留有界的原始行，便于管理员定位问题。
#[derive(Clone)]
pub struct AdminForumWarning {
    pub line: usize,
    pub code: String,
    pub message: String,
    pub text: Option<String>,
    pub post_id: Option<String>,
}

impl fmt::Debug for AdminForumWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumWarning")
            .field("line", &self.line)
            .field("text_bytes", &self.text.as_ref().map(String::len))
            .field("post_id_present", &self.post_id.is_some())
            .finish()
    }
}

#[derive(Clone)]
pub struct AdminForumPreviewResult {
    /// 目录首帖的远端稳定 post ID；Admin HTTP 契约无需暴露该内部写入证明。
    pub catalog_post_id: String,
    /// 目录首帖的规范纯文本；只用于固定楼层归属校验，不进入 Admin HTTP 契约。
    pub catalog_post_text: String,
    pub catalog: AdminForumCatalog,
    pub posts: Vec<AdminForumBidPost>,
    pub warnings: Vec<AdminForumWarning>,
}

// 论坛输入可能含装备 URL 和 key；调试输出只展示结构与长度，不输出任何原始文本。
impl fmt::Debug for AdminForumPreviewResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminForumPreviewResult")
            .field("catalog_post_text_bytes", &self.catalog_post_text.len())
            .field("catalog", &self.catalog)
            .field("post_count", &self.posts.len())
            .field("warning_count", &self.warnings.len())
            .finish()
    }
}

impl AdminForumPreviewResult {
    /// 校验解析器返回的边界，防止异常输入生成无法安全输出的投影。
    pub fn validate(&self) -> Result<(), AdminForumPreviewError> {
        if self.catalog.lots.is_empty()
            || !valid_text(&self.catalog_post_id, 256)
            || self.catalog_post_text.is_empty()
            || self.catalog_post_text.len() > 1_000_000
            || self.catalog.lots.len() > 10_000
            || self.posts.len() > 10_000
            || self.warnings.len() > 20_000
            || self.catalog.categories.iter().any(|value| !valid_text(value, 128))
            || self.catalog.lots.iter().any(|lot| {
                !valid_text(&lot.code, 16)
                    || !valid_text(&lot.category, 128)
                    || lot.description.len() > 16_384
                    || lot.seller.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || lot.start_price <= 0
                    || lot.current_bidder.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || lot.current_price.is_some_and(|value| value <= 0)
            })
            || self.posts.iter().any(|post| {
                !valid_text(&post.post.id, 256)
                    || post.post.text.len() > 1_000_000
                    || post.post.author_id.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || post.post.author.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || post.commands.len() > 10_000
            })
        {
            return Err(AdminForumPreviewError::ParseFailed);
        }
        Ok(())
    }
}

/// Admin 论坛预览错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminForumPreviewError {
    #[error("论坛预览输入无效")]
    InvalidInput,
    #[error("论坛拍卖内容无法解析")]
    ParseFailed,
}

pub trait AdminForumPreviewPort: Send + Sync {
    fn preview(
        &self,
        command: AdminForumPreviewCommand,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult>;
}

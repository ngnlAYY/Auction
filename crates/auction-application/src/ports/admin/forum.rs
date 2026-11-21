//! 论坛拍卖主题的解析结果模型与解析错误。
//! 由 `auction-integrations` 的论坛客户端解析已读取的主题页面后产出，供回帖导入与目录核对
//! 使用；不写数据库，也不把解析成功当作已经发布的论坛事实。

use std::fmt;

use auction_domain::valid_text;
use thiserror::Error;

const MAX_FORUM_CATEGORY_COUNT: usize = 256;
const MAX_FORUM_PREVIEW_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_FORUM_PREVIEW_COMMANDS: usize = 20_000;
const MAX_FORUM_PREVIEW_WARNINGS: usize = 20_000;
const MAX_FORUM_NESTED_ITEMS: usize = 1_000;
const MAX_FORUM_COMMAND_LOT_CODES: usize = 64;

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
        if self.catalog.categories.len() > MAX_FORUM_CATEGORY_COUNT
            || self.catalog.lots.len() > 10_000
            || self.posts.len() > 10_000
            || self.warnings.len() > MAX_FORUM_PREVIEW_WARNINGS
            || self.catalog.warnings.len() > MAX_FORUM_PREVIEW_WARNINGS
            || self.posts.iter().any(|post| {
                post.commands.len() > MAX_FORUM_NESTED_ITEMS
                    || post.warnings.len() > MAX_FORUM_NESTED_ITEMS
            })
        {
            return Err(AdminForumPreviewError::ParseFailed);
        }
        let command_count = self
            .posts
            .iter()
            .fold(0_usize, |count, post| count.saturating_add(post.commands.len()));
        let warning_count = self.posts.iter().fold(
            self.catalog.warnings.len().saturating_add(self.warnings.len()),
            |count, post| count.saturating_add(post.warnings.len()),
        );
        if command_count > MAX_FORUM_PREVIEW_COMMANDS || warning_count > MAX_FORUM_PREVIEW_WARNINGS
        {
            return Err(AdminForumPreviewError::ParseFailed);
        }
        let mut text_bytes = 0_usize;
        add_text_bytes(&mut text_bytes, &self.catalog_post_id);
        add_text_bytes(&mut text_bytes, &self.catalog_post_text);
        for category in &self.catalog.categories {
            add_text_bytes(&mut text_bytes, category);
        }
        for lot in &self.catalog.lots {
            add_text_bytes(&mut text_bytes, &lot.code);
            add_text_bytes(&mut text_bytes, &lot.category);
            add_text_bytes(&mut text_bytes, &lot.description);
            add_optional_text_bytes(&mut text_bytes, lot.seller.as_deref());
            add_optional_text_bytes(&mut text_bytes, lot.current_bidder.as_deref());
        }
        for warning in &self.catalog.warnings {
            add_warning_text_bytes(&mut text_bytes, warning);
        }
        for warning in &self.warnings {
            add_warning_text_bytes(&mut text_bytes, warning);
        }
        for post in &self.posts {
            add_text_bytes(&mut text_bytes, &post.post.id);
            add_optional_text_bytes(&mut text_bytes, post.post.author_id.as_deref());
            add_optional_text_bytes(&mut text_bytes, post.post.author.as_deref());
            add_text_bytes(&mut text_bytes, &post.post.text);
            for command in &post.commands {
                add_text_bytes(&mut text_bytes, &command.text);
                add_optional_text_bytes(&mut text_bytes, command.actor.as_deref());
                add_text_bytes(&mut text_bytes, &command.kind);
                add_optional_text_bytes(&mut text_bytes, command.notify.as_deref());
                for code in &command.lot_codes {
                    add_text_bytes(&mut text_bytes, code);
                }
            }
            for warning in &post.warnings {
                add_warning_text_bytes(&mut text_bytes, warning);
            }
        }
        if self.catalog.lots.is_empty()
            || !valid_text(&self.catalog_post_id, 256)
            || self.catalog_post_text.is_empty()
            || self.catalog_post_text.len() > 1_000_000
            || self.catalog.categories.len() > MAX_FORUM_CATEGORY_COUNT
            || self.catalog.lots.len() > 10_000
            || self.posts.len() > 10_000
            || command_count > MAX_FORUM_PREVIEW_COMMANDS
            || warning_count > MAX_FORUM_PREVIEW_WARNINGS
            || text_bytes > MAX_FORUM_PREVIEW_TEXT_BYTES
            || self.catalog.categories.iter().any(|value| !valid_text(value, 128))
            || self.catalog.lots.iter().any(|lot| {
                !valid_text(&lot.code, 16)
                    || !valid_text(&lot.category, 128)
                    || !valid_text(&lot.description, 16_384)
                    || lot.seller.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || lot.start_price <= 0
                    || lot.current_bidder.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || lot.current_price.is_some_and(|value| value <= 0)
            })
            || self.catalog.warnings.iter().any(|warning| !valid_forum_warning(warning))
            || self.warnings.iter().any(|warning| !valid_forum_warning(warning))
            || self.posts.iter().any(|post| {
                !valid_text(&post.post.id, 256)
                    || post.post.text.len() > 1_000_000
                    || post.post.author_id.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || post.post.author.as_deref().is_some_and(|value| !valid_text(value, 256))
                    || post.commands.len() > MAX_FORUM_NESTED_ITEMS
                    || post.warnings.len() > MAX_FORUM_NESTED_ITEMS
                    || post.commands.iter().any(|command| !valid_forum_command(command))
                    || post.warnings.iter().any(|warning| !valid_forum_warning(warning))
            })
        {
            return Err(AdminForumPreviewError::ParseFailed);
        }
        Ok(())
    }
}

fn valid_forum_command(command: &AdminForumCommand) -> bool {
    command.line > 0
        && valid_text(&command.text, 16_384)
        && command.actor.as_deref().is_none_or(|value| valid_text(value, 256))
        && command.lot_codes.len() <= MAX_FORUM_COMMAND_LOT_CODES
        && command.lot_codes.iter().all(|code| valid_text(code, 16))
        && valid_text(&command.kind, 64)
        && command.amount.is_none_or(|amount| amount > 0)
        && command.notify.as_deref().is_none_or(|value| valid_text(value, 64))
}

fn valid_forum_warning(warning: &AdminForumWarning) -> bool {
    (warning.line > 0 || warning.code == "edited_post_ignored" && warning.post_id.is_some())
        && valid_text(&warning.code, 128)
        && valid_text(&warning.message, 1_024)
        && warning.text.as_deref().is_none_or(|value| valid_text(value, 16_384))
        && warning.post_id.as_deref().is_none_or(|value| valid_text(value, 256))
}

fn add_text_bytes(total: &mut usize, value: &str) {
    *total = total.saturating_add(value.len());
}

fn add_optional_text_bytes(total: &mut usize, value: Option<&str>) {
    if let Some(value) = value {
        add_text_bytes(total, value);
    }
}

fn add_warning_text_bytes(total: &mut usize, warning: &AdminForumWarning) {
    add_text_bytes(total, &warning.code);
    add_text_bytes(total, &warning.message);
    add_optional_text_bytes(total, warning.text.as_deref());
    add_optional_text_bytes(total, warning.post_id.as_deref());
}

/// Admin 论坛预览错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminForumPreviewError {
    #[error("论坛拍卖内容无法解析")]
    ParseFailed,
}

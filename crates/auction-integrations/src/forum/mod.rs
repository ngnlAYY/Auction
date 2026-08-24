//! 论坛拍卖预览解析适配器。
//!
//! 该模块只消费 Admin 提交的 HTML/文本快照并返回内存中的安全预览，不连接论坛、不写本地
//! 状态，也不把解析成功误报为远程发布成功。语法和金额边界沿用 Auction-BF 论坛契约。

mod parser;
mod render;

pub use auction_application::ForumRenderError;
pub use render::{ForumThreadRenderer, render_auction_thread};

use auction_application::{
    AdminForumPreviewCommand, AdminForumPreviewError, AdminForumPreviewPort,
    AdminForumPreviewResult, ApplicationFuture,
};
use parser::{parse_html, parse_thread_text};

/// 无状态的论坛预览解析器。
#[derive(Debug, Default, Clone, Copy)]
pub struct ForumPreviewAdapter;

impl ForumPreviewAdapter {
    /// 建立不持有网络或数据库状态的解析器。
    pub const fn new() -> Self {
        Self
    }

    /// 解析已读取的论坛主题 HTML，并复用 Admin 预览的契约校验。
    ///
    /// 外部网关只负责受限读取和认证；帖子语义仍由本适配器统一解释，避免远程读取路径
    /// 与本地预览路径产生两套命令语法或安全边界。
    pub fn parse_html_document(
        &self,
        input: &str,
    ) -> Result<AdminForumPreviewResult, AdminForumPreviewError> {
        let result = parse_html(input).map_err(|_| AdminForumPreviewError::ParseFailed)?;
        result.validate()?;
        Ok(result)
    }
}

impl AdminForumPreviewPort for ForumPreviewAdapter {
    fn preview(
        &self,
        command: AdminForumPreviewCommand,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult> {
        Box::pin(async move {
            command.validate()?;
            let result = if let Some(html) = command.html {
                parse_html(&html).map_err(|_| AdminForumPreviewError::ParseFailed)?
            } else if let Some(text) = command.text {
                parse_thread_text(&text).map_err(|_| AdminForumPreviewError::ParseFailed)?
            } else {
                return Err(AdminForumPreviewError::InvalidInput.into());
            };
            result.validate()?;
            Ok(result)
        })
    }
}

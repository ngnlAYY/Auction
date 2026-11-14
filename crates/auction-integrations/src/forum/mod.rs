//! 论坛拍卖主题解析适配器；只消费论坛客户端读取的受限页面事实，
//! 不连接论坛、不写本地状态，也不把解析成功误报为远程发布成功。

mod parser;
mod render;

pub use auction_application::forum::ForumRenderError;
pub use render::ForumThreadRenderer;

use auction_application::admin::{AdminForumPreviewError, AdminForumPreviewResult};
pub(crate) use parser::ParsedHtmlPage;

#[derive(Debug, Default, Clone, Copy)]
pub struct ForumPreviewAdapter;

impl ForumPreviewAdapter {
    /// 建立不持有网络或数据库状态的解析器。
    pub const fn new() -> Self {
        Self
    }

    /// 单次构建 DOM 并提取分页校验与帖子读取共用的受限页面事实。
    pub(crate) fn parse_html_page(
        &self,
        input: &str,
    ) -> Result<ParsedHtmlPage, AdminForumPreviewError> {
        parser::parse_html_page(input).map_err(|_| AdminForumPreviewError::ParseFailed)
    }

    /// 按论坛页码顺序合并已经解析的页面事实，校验跨页 post 身份后解释目录和普通出价。
    /// 每个远端 post ID 在一个主题中必须唯一；重复 ID 代表分页协议漂移，不能静默去重后继续
    /// 应用出价，否则同一命令可能失去稳定楼层身份。
    pub(crate) fn parse_html_pages(
        &self,
        pages: Vec<ParsedHtmlPage>,
    ) -> Result<AdminForumPreviewResult, AdminForumPreviewError> {
        let result =
            parser::parse_html_pages(pages).map_err(|_| AdminForumPreviewError::ParseFailed)?;
        result.validate()?;
        Ok(result)
    }
}

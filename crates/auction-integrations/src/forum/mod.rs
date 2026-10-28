//! 论坛拍卖预览解析适配器；只消费 Admin 提交的 HTML/文本快照，不连接论坛、
//! 不写本地状态，也不把解析成功误报为远程发布成功。

mod parser;
mod render;

pub use auction_application::ForumRenderError;
pub use render::ForumThreadRenderer;

use auction_application::{
    AdminForumPreviewCommand, AdminForumPreviewError, AdminForumPreviewPort,
    AdminForumPreviewResult, ApplicationFuture,
};
pub(crate) use parser::ParsedHtmlPage;
use parser::{parse_html, parse_html_documents, parse_thread_text};

#[derive(Debug, Default, Clone, Copy)]
pub struct ForumPreviewAdapter;

impl ForumPreviewAdapter {
    /// 建立不持有网络或数据库状态的解析器。
    pub const fn new() -> Self {
        Self
    }

    /// 按论坛页码顺序合并一个主题的全部 HTML 页面，再统一解析目录和出价命令。
    /// 每个远端 post ID 在一个主题中必须唯一；重复 ID 代表分页协议漂移，不能静默去重后继续
    /// 应用出价，否则同一命令可能失去稳定楼层身份。
    pub fn parse_html_documents(
        &self,
        inputs: &[&str],
    ) -> Result<AdminForumPreviewResult, AdminForumPreviewError> {
        if inputs.is_empty() {
            return Err(AdminForumPreviewError::ParseFailed);
        }
        let result =
            parse_html_documents(inputs).map_err(|_| AdminForumPreviewError::ParseFailed)?;
        result.validate()?;
        Ok(result)
    }

    /// 单次构建 DOM 并提取分页校验与帖子读取共用的受限页面事实。
    pub(crate) fn parse_html_page(
        &self,
        input: &str,
    ) -> Result<ParsedHtmlPage, AdminForumPreviewError> {
        parser::parse_html_page(input).map_err(|_| AdminForumPreviewError::ParseFailed)
    }

    /// 合并已经解析的页面事实，校验跨页 post 身份后解释目录和普通出价。
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

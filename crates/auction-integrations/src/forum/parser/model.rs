//! 论坛预览解析过程中的内部模型。

use std::fmt;

use auction_application::admin::{AdminForumLot, AdminForumWarning};

pub type CatalogData = (Vec<String>, Vec<AdminForumLot>);
pub type CatalogParse = (CatalogData, Vec<AdminForumWarning>);

#[derive(Clone)]
pub struct ParsedPost {
    pub(super) id: String,
    /// 论坛 `showuser` 的稳定数字账号键。
    pub(super) author_id: Option<String>,
    /// 论坛用户名，也是 HentaiVerse/MoogleMail 使用的可变玩家名称。
    pub(super) author: Option<String>,
    pub(super) text: String,
    pub(super) edited: bool,
}

impl fmt::Debug for ParsedPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedPost")
            .field("author_id_present", &self.author_id.is_some())
            .field("author_present", &self.author.is_some())
            .field("text_bytes", &self.text.len())
            .field("edited", &self.edited)
            .finish()
    }
}

/// 单次 DOM 解析得到的论坛页面事实；不保留原始 HTML、Cookie 或表单 token。
pub(crate) struct ParsedHtmlPage {
    posts: Vec<ParsedPost>,
    navigation_references: Vec<String>,
}

impl fmt::Debug for ParsedHtmlPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedHtmlPage")
            .field("post_count", &self.posts.len())
            .field("navigation_reference_count", &self.navigation_references.len())
            .finish()
    }
}

impl ParsedHtmlPage {
    pub(super) fn new(posts: Vec<ParsedPost>, navigation_references: Vec<String>) -> Self {
        Self { posts, navigation_references }
    }

    pub(crate) fn post_ids(&self) -> impl Iterator<Item = &str> {
        self.posts.iter().map(|post| post.id.as_str())
    }

    /// 本页帖子数；用于把分页偏移换算成主题内的绝对楼层。
    pub(crate) fn post_count(&self) -> usize {
        self.posts.len()
    }

    pub(crate) fn navigation_references(&self) -> impl Iterator<Item = &str> {
        self.navigation_references.iter().map(String::as_str)
    }

    pub(super) fn into_posts(self) -> Vec<ParsedPost> {
        self.posts
    }
}

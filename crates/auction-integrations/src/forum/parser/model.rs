//! 论坛预览解析过程中的内部模型。

use auction_application::{AdminForumLot, AdminForumWarning};

pub type CatalogData = (Vec<String>, Vec<AdminForumLot>);
pub type CatalogParse = (CatalogData, Vec<AdminForumWarning>);

#[derive(Debug, Clone)]
pub struct ParsedPost {
    pub(super) id: String,
    pub(super) author_id: Option<String>,
    pub(super) author: Option<String>,
    pub(super) text: String,
    pub(super) edited: bool,
}

/// 单次 DOM 解析得到的论坛页面事实；不保留原始 HTML、Cookie 或表单 token。
#[derive(Debug)]
pub(crate) struct ParsedHtmlPage {
    posts: Vec<ParsedPost>,
    navigation_references: Vec<String>,
}

impl ParsedHtmlPage {
    pub(super) fn new(posts: Vec<ParsedPost>, navigation_references: Vec<String>) -> Self {
        Self { posts, navigation_references }
    }

    pub(crate) fn post_ids(&self) -> impl Iterator<Item = &str> {
        self.posts.iter().map(|post| post.id.as_str())
    }

    pub(crate) fn navigation_references(&self) -> impl Iterator<Item = &str> {
        self.navigation_references.iter().map(String::as_str)
    }

    pub(super) fn into_posts(self) -> Vec<ParsedPost> {
        self.posts
    }
}

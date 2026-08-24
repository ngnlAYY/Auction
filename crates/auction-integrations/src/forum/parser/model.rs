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

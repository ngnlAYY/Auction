//! 论坛预览的语法分层与结果编排。

use std::collections::HashSet;

use auction_application::{
    AdminForumCatalog, AdminForumPreviewResult, FORUM_CATALOG_PAGE_SIZE, MAX_FORUM_CATALOG_PAGES,
    forum_catalog_page_marker,
};
use auction_domain::AuctionId;
use uuid::Uuid;

mod catalog;
mod commands;
mod helpers;
mod html;
mod model;
mod text;

pub(super) use html::parse_html;
pub(crate) use model::ParsedHtmlPage;
pub(super) use model::{CatalogParse, ParsedPost};
pub(super) use text::parse_thread_text;

pub(super) const DEFAULT_START_PRICE: i64 = 50_000;
pub(super) const CREDIT_GRID: i64 = 1_000;
pub(super) const CATALOG_CATEGORIES: [&str; 7] =
    ["One-Handed", "Two-Handed", "Staff", "Shield", "Cloth", "Light", "Heavy"];

pub(super) fn parse_html_documents(inputs: &[&str]) -> Result<AdminForumPreviewResult, ()> {
    if inputs.is_empty() {
        return Err(());
    }
    let pages =
        inputs.iter().map(|input| html::extract_html_page(input)).collect::<Result<Vec<_>, _>>()?;
    parse_html_pages(pages)
}

pub(crate) fn parse_html_page(input: &str) -> Result<ParsedHtmlPage, ()> {
    html::extract_html_page(input)
}

pub(crate) fn parse_html_pages(pages: Vec<ParsedHtmlPage>) -> Result<AdminForumPreviewResult, ()> {
    if pages.is_empty() {
        return Err(());
    }
    let mut posts = Vec::<ParsedPost>::new();
    for page in pages {
        for post in page.into_posts() {
            if !html::canonical_post_id(&post.id)
                // canonical 数字 ID 严格递增已经排除全部重复，无需复制 ID 维护第二个集合。
                || posts
                    .last()
                    .is_some_and(|previous| !decimal_id_precedes(&previous.id, &post.id))
            {
                return Err(());
            }
            posts.push(post);
        }
    }
    parse_thread(posts)
}

fn decimal_id_precedes(left: &str, right: &str) -> bool {
    left.len() < right.len() || left.len() == right.len() && left < right
}

pub(super) fn parse_thread(posts: Vec<ParsedPost>) -> Result<AdminForumPreviewResult, ()> {
    parse_thread_with_identity(posts, true)
}

/// 纯文本预览没有数字作者证明，只比较可选作者标签；该入口不得用于 Worker 远端导入。
fn parse_thread_with_identity(
    posts: Vec<ParsedPost>,
    require_author_id: bool,
) -> Result<AdminForumPreviewResult, ()> {
    let Some(catalog_post) = posts.first() else {
        return Err(());
    };
    let catalog_post_id = catalog_post.id.clone();
    let catalog_post_text = catalog_post.text.clone();
    let (mut catalog, mut catalog_warnings) = catalog::parse_catalog(&catalog_post.text)?;
    let auction_id = catalog_post.text.split_whitespace().find_map(|token| {
        token
            .strip_prefix("auction-catalog-v1:")
            .and_then(|value| Uuid::parse_str(value).ok())
            .filter(|value| !value.is_nil())
            .map(AuctionId::from_uuid)
    });
    if let Some(auction_id) = auction_id {
        let mut codes = catalog.1.iter().map(|lot| lot.code.clone()).collect::<HashSet<_>>();
        for (index, post) in posts.iter().enumerate().skip(1) {
            let marker = forum_catalog_page_marker(auction_id, index + 1);
            if !post.text.split_whitespace().any(|token| token == marker) {
                break;
            }
            // 只合并紧邻首帖、同作者且有匹配拍卖/楼层标记的目录；不按正文跳过用户命令。
            // Worker 导入前还必须核对持久化 post ID，预览解析本身不构成写入授权。
            if index >= MAX_FORUM_CATALOG_PAGES
                || (require_author_id
                    && (catalog_post.author_id.is_none()
                        || post.author_id != catalog_post.author_id))
                || (!require_author_id && post.author != catalog_post.author)
            {
                return Err(());
            }
            let (page, mut warnings) = catalog::parse_catalog(&post.text)?;
            if page.1.len() > FORUM_CATALOG_PAGE_SIZE {
                return Err(());
            }
            for category in page.0 {
                if !catalog.0.contains(&category) {
                    catalog.0.push(category);
                }
            }
            for lot in page.1 {
                if !codes.insert(lot.code.clone()) {
                    return Err(());
                }
                catalog.1.push(lot);
            }
            for warning in &mut warnings {
                warning.post_id = Some(post.id.clone());
            }
            catalog_warnings.extend(warnings);
        }
    }
    let mut result = AdminForumPreviewResult {
        catalog_post_id,
        catalog_post_text,
        catalog: AdminForumCatalog {
            categories: catalog.0,
            lots: catalog.1,
            warnings: catalog_warnings.clone(),
        },
        posts: Vec::with_capacity(posts.len().saturating_sub(1)),
        warnings: catalog_warnings,
    };
    // 完整目录只建一次借用索引；每条命令无需线性扫描全部拍品。
    let lot_codes = result.catalog.lots.iter().map(|lot| lot.code.as_str()).collect();
    for post in posts.into_iter().skip(1) {
        let (parsed, warnings) = commands::parse_bid_post(post, &lot_codes);
        result.warnings.extend(warnings);
        result.posts.push(parsed);
    }
    helpers::sort_warnings(&mut result.warnings);
    Ok(result)
}

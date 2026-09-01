//! 论坛预览的语法分层与结果编排。

use std::collections::HashSet;

use auction_application::{AdminForumCatalog, AdminForumPreviewResult};

#[path = "catalog.rs"]
mod catalog;
#[path = "commands.rs"]
mod commands;
#[path = "helpers.rs"]
mod helpers;
#[path = "html.rs"]
mod html;
#[path = "model.rs"]
mod model;
#[path = "text.rs"]
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
    let mut seen_post_ids = HashSet::new();
    let mut posts = Vec::new();
    let mut previous_post_id = None;
    for page in pages {
        for post in page.into_posts() {
            if !html::canonical_post_id(&post.id)
                || previous_post_id
                    .as_deref()
                    .is_some_and(|previous| !decimal_id_precedes(previous, &post.id))
                || !seen_post_ids.insert(post.id.clone())
            {
                return Err(());
            }
            previous_post_id = Some(post.id.clone());
            posts.push(post);
        }
    }
    parse_thread(posts)
}

fn decimal_id_precedes(left: &str, right: &str) -> bool {
    left.len() < right.len() || left.len() == right.len() && left < right
}

pub(super) fn parse_thread(posts: Vec<ParsedPost>) -> Result<AdminForumPreviewResult, ()> {
    let Some(catalog_post) = posts.first() else {
        return Err(());
    };
    let catalog_post_id = catalog_post.id.clone();
    let catalog_post_text = catalog_post.text.clone();
    let (catalog, catalog_warnings) = catalog::parse_catalog(&catalog_post.text)?;
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
    for post in posts.into_iter().skip(1) {
        let parsed = commands::parse_bid_post(post, &result.catalog.lots);
        result.warnings.extend(parsed.1.iter().cloned());
        result.posts.push(parsed.0);
    }
    helpers::sort_warnings(&mut result.warnings);
    Ok(result)
}

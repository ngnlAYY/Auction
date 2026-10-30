//! 论坛目录续页的身份校验与 PostgreSQL 镜像持久化；主记录锁和 CAS 由调用方持有。

use std::collections::HashSet;

use crate::database::storage_error;
use auction_application::{ForumCatalogPage, MAX_FORUM_CATALOG_PAGES};

/// 所有固定帖身份必须互异且为规范十进制；续页顺序由数组位置表示，不能按 ID 排楼。
pub(super) fn valid_page_ids<'a>(
    first_post_id: &'a str,
    system_post_id: &'a str,
    continuation: impl Iterator<Item = &'a str>,
) -> bool {
    let mut seen = HashSet::new();
    for post_id in
        std::iter::once(first_post_id).chain(std::iter::once(system_post_id)).chain(continuation)
    {
        if !auction_domain::valid_positive_decimal(post_id, 32) || !seen.insert(post_id) {
            return false;
        }
    }
    seen.len() <= MAX_FORUM_CATALOG_PAGES + 1
}

pub(super) fn valid_pages(
    first_post_id: &str,
    system_post_id: &str,
    pages: &[ForumCatalogPage],
) -> bool {
    valid_page_ids(first_post_id, system_post_id, pages.iter().map(|page| page.post_id.as_str()))
        && pages.iter().all(|page| {
            !page.body.trim().is_empty()
                && page.body.len() <= 65_531
                && !page.body.chars().any(|character| {
                    character.is_control() && !matches!(character, '\n' | '\r' | '\t')
                })
        })
}

#[cfg(feature = "postgres")]
use auction_application::{
    ApplicationError, ForumPublishedLayout, ForumRenderedThread, ForumReplyTopic,
};
/// 数据库布局只接受冻结闭集，不以当前件数或远端文本推断历史方案。
#[cfg(feature = "postgres")]
pub(super) fn parse_layout(
    value: &str,
) -> Result<auction_application::ForumCatalogLayout, ApplicationError> {
    match value {
        "legacy_single" => Ok(auction_application::ForumCatalogLayout::LegacySingle),
        "paged_v1" => Ok(auction_application::ForumCatalogLayout::PagedV1),
        _ => Err(storage_error("论坛目录布局不在闭集内")),
    }
}

#[cfg(feature = "postgres")]
pub(super) async fn read_pages(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: uuid::Uuid,
    page_count: i32,
) -> Result<Vec<ForumCatalogPage>, ApplicationError> {
    use sqlx::Row;

    if !(1..=MAX_FORUM_CATALOG_PAGES as i32).contains(&page_count) {
        return Err(storage_error("论坛目录页数超出范围"));
    }
    let rows = sqlx::query(
        "SELECT post_number, post_id, body FROM public.auction_forum_catalog_pages
         WHERE auction_id = $1 ORDER BY post_number",
    )
    .bind(auction_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取论坛目录续页失败"))?;
    if rows.len() + 1 != page_count as usize {
        return Err(storage_error("论坛目录续页数量与发布证明不一致"));
    }
    let mut pages = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        let post_number: i32 =
            row.try_get("post_number").map_err(|_| storage_error("读取论坛目录续页楼层失败"))?;
        if post_number as usize != index + 2 {
            return Err(storage_error("论坛目录续页楼层不连续"));
        }
        pages.push(ForumCatalogPage {
            post_id: row
                .try_get("post_id")
                .map_err(|_| storage_error("读取论坛目录续页身份失败"))?,
            body: row.try_get("body").map_err(|_| storage_error("读取论坛目录续页正文失败"))?,
        });
    }
    Ok(pages)
}

/// 发布事务可重放同一组身份，不得借冲突更新把旧固定帖改指向另一远端帖子。
#[cfg(feature = "postgres")]
pub(super) async fn publish_pages(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auction_id: uuid::Uuid,
    rendered: &ForumRenderedThread,
    published: &ForumPublishedLayout,
) -> Result<(), ApplicationError> {
    for (index, (post_id, page)) in published
        .catalog_continuation_post_ids
        .iter()
        .zip(&rendered.catalog_continuation)
        .enumerate()
    {
        let result = sqlx::query(
            "INSERT INTO public.auction_forum_catalog_pages (auction_id, post_number, post_id, body)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (auction_id, post_number) DO UPDATE SET body = EXCLUDED.body
             WHERE auction_forum_catalog_pages.post_id = EXCLUDED.post_id",
        )
        .bind(auction_id)
        .bind((index + 2) as i32)
        .bind(post_id)
        .bind(&page.body)
        .execute(&mut **transaction)
        .await
        .map_err(|_| storage_error("写入论坛目录续页发布证明失败"))?;
        if result.rows_affected() != 1 {
            return Err(storage_error("论坛目录续页身份已变化"));
        }
    }
    Ok(())
}

/// 调用方的主记录 CAS 及全部续页写入必须同事务提交，任何一页缺失都回滚本地证明。
#[cfg(feature = "postgres")]
pub(super) async fn update_pages(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    topic: &ForumReplyTopic,
    rendered: &ForumRenderedThread,
) -> Result<(), ApplicationError> {
    if topic.catalog_continuation.len() != rendered.catalog_continuation.len() {
        return Err(storage_error("论坛目录页数不能在刷新时改变"));
    }
    for (index, (previous, page)) in
        topic.catalog_continuation.iter().zip(&rendered.catalog_continuation).enumerate()
    {
        let result = sqlx::query(
            "UPDATE public.auction_forum_catalog_pages SET body = $4
             WHERE auction_id = $1 AND post_number = $2 AND post_id = $3",
        )
        .bind(topic.auction_id.as_uuid())
        .bind((index + 2) as i32)
        .bind(&previous.post_id)
        .bind(&page.body)
        .execute(&mut **transaction)
        .await
        .map_err(|_| storage_error("更新论坛目录续页正文证明失败"))?;
        if result.rows_affected() != 1 {
            return Err(storage_error("论坛目录续页身份或楼层已变化"));
        }
    }
    Ok(())
}

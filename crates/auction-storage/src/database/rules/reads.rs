//! Public 与 Admin 规则文档的 PostgreSQL 只读投影。

use super::*;
use crate::database::storage_error;

impl PublicRulesReader for DatabaseStore {
    fn get_public_rules(
        &self,
        request: PublicRulesRequest,
    ) -> ApplicationFuture<'_, Option<PublicRulesDocument>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => get_postgres(store, request.locale).await,
            }
        })
    }
}

impl AdminRulesReader for DatabaseStore {
    fn get_admin_rules(
        &self,
        request: AdminRulesRequest,
    ) -> ApplicationFuture<'_, Option<AdminRulesDocument>> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::AdminRules)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => get_admin_postgres(store, request.locale).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn get_admin_postgres(
    store: &super::PostgresStore,
    locale: String,
) -> Result<Option<AdminRulesDocument>, ApplicationError> {
    let row = sqlx::query(
        "SELECT locale, revision, title, markdown, published_by, published_at
         FROM public.rules_documents WHERE locale = $1",
    )
    .bind(locale)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL Admin 规则文档失败"))?;
    row.map(|row| {
        Ok(AdminRulesDocument {
            locale: row.try_get("locale").map_err(|_| storage_error("读取规则语言失败"))?,
            revision: row.try_get("revision").map_err(|_| storage_error("读取规则版本失败"))?,
            title: row.try_get("title").map_err(|_| storage_error("读取规则标题失败"))?,
            markdown: row.try_get("markdown").map_err(|_| storage_error("读取规则正文失败"))?,
            published_by: row
                .try_get("published_by")
                .map_err(|_| storage_error("读取规则发布者失败"))?,
            published_at: row
                .try_get("published_at")
                .map_err(|_| storage_error("读取规则发布时间失败"))?,
        })
    })
    .transpose()
}

#[cfg(feature = "postgres")]
async fn get_postgres(
    store: &super::PostgresStore,
    locale: String,
) -> Result<Option<PublicRulesDocument>, ApplicationError> {
    let row = sqlx::query(
        "SELECT locale, revision, title, markdown, published_at
         FROM public.rules_documents
         WHERE locale = $1",
    )
    .bind(locale)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 规则文档失败"))?;

    row.map(|row| {
        Ok(PublicRulesDocument {
            locale: row.try_get("locale").map_err(|_| storage_error("读取规则语言失败"))?,
            revision: row.try_get("revision").map_err(|_| storage_error("读取规则版本失败"))?,
            title: row.try_get("title").map_err(|_| storage_error("读取规则标题失败"))?,
            markdown: row.try_get("markdown").map_err(|_| storage_error("读取规则正文失败"))?,
            published_at: row
                .try_get("published_at")
                .map_err(|_| storage_error("读取规则发布时间失败"))?,
        })
    })
    .transpose()
}

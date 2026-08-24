//! Public 与 Admin 规则文档读取及 Lite 时间映射。

use super::*;

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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => get_sqlite(store, request.locale).await,
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => get_admin_sqlite(store, request.locale).await,
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

#[cfg(feature = "sqlite")]
async fn get_admin_sqlite(
    store: &super::SqliteStore,
    locale: String,
) -> Result<Option<AdminRulesDocument>, ApplicationError> {
    let row = sqlx::query(
        "SELECT locale, revision, title, markdown, published_by, published_at
         FROM rules_documents WHERE locale = ?",
    )
    .bind(locale)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite Admin 规则文档失败"))?;
    row.map(|row| {
        Ok(AdminRulesDocument {
            locale: row.try_get("locale").map_err(|_| storage_error("读取规则语言失败"))?,
            revision: row.try_get("revision").map_err(|_| storage_error("读取规则版本失败"))?,
            title: row.try_get("title").map_err(|_| storage_error("读取规则标题失败"))?,
            markdown: row.try_get("markdown").map_err(|_| storage_error("读取规则正文失败"))?,
            published_by: row
                .try_get("published_by")
                .map_err(|_| storage_error("读取规则发布者失败"))?,
            published_at: parse_timestamp(
                row.try_get("published_at").map_err(|_| storage_error("读取规则发布时间失败"))?,
            )?,
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

#[cfg(feature = "sqlite")]
async fn get_sqlite(
    store: &super::SqliteStore,
    locale: String,
) -> Result<Option<PublicRulesDocument>, ApplicationError> {
    let row = sqlx::query(
        "SELECT locale, revision, title, markdown, published_at
         FROM rules_documents
         WHERE locale = ?",
    )
    .bind(locale)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite 规则文档失败"))?;

    row.map(|row| {
        let published_at = parse_timestamp(
            row.try_get("published_at").map_err(|_| storage_error("读取 Lite 规则发布时间失败"))?,
        )?;
        Ok(PublicRulesDocument {
            locale: row.try_get("locale").map_err(|_| storage_error("读取规则语言失败"))?,
            revision: row.try_get("revision").map_err(|_| storage_error("读取规则版本失败"))?,
            title: row.try_get("title").map_err(|_| storage_error("读取规则标题失败"))?,
            markdown: row.try_get("markdown").map_err(|_| storage_error("读取规则正文失败"))?,
            published_at,
        })
    })
    .transpose()
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error("Lite 规则发布时间格式无效"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}

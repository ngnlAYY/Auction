//! Public 与 Admin 站点文档（规则、关于）的 PostgreSQL 只读投影。
//! 表名沿用 `rules_documents`；文档身份是（kind，locale）。

use auction_application::admin::{AdminRulesDocument, AdminRulesReader, AdminRulesRequest};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::public::{
    PublicRulesDocument, PublicRulesReader, PublicRulesRequest, SiteDocumentKind,
};
use sqlx::Row;

use crate::database::DatabaseStore;
use crate::database::storage_error;
use crate::database::storage_error_from;

impl PublicRulesReader for DatabaseStore {
    fn get_public_rules(
        &self,
        request: PublicRulesRequest,
    ) -> ApplicationFuture<'_, Option<PublicRulesDocument>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            get_postgres(self, request.kind, request.locale).await
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
            get_admin_postgres(self, request.kind, request.locale).await
        })
    }
}

async fn get_admin_postgres(
    store: &super::DatabaseStore,
    kind: SiteDocumentKind,
    locale: String,
) -> Result<Option<AdminRulesDocument>, ApplicationError> {
    let row = sqlx::query(
        "SELECT locale, revision, title, markdown, published_by, published_at
         FROM public.rules_documents WHERE kind = $1 AND locale = $2",
    )
    .bind(kind.as_str())
    .bind(locale)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL Admin 规则文档失败"))?;
    row.map(|row| {
        Ok(AdminRulesDocument {
            locale: row
                .try_get("locale")
                .map_err(|error| storage_error_from(error, "读取规则语言失败"))?,
            revision: row
                .try_get("revision")
                .map_err(|error| storage_error_from(error, "读取规则版本失败"))?,
            title: row
                .try_get("title")
                .map_err(|error| storage_error_from(error, "读取规则标题失败"))?,
            markdown: row
                .try_get("markdown")
                .map_err(|error| storage_error_from(error, "读取规则正文失败"))?,
            published_by: row
                .try_get("published_by")
                .map_err(|error| storage_error_from(error, "读取规则发布者失败"))?,
            published_at: row
                .try_get("published_at")
                .map_err(|error| storage_error_from(error, "读取规则发布时间失败"))?,
        })
    })
    .transpose()
}

async fn get_postgres(
    store: &super::DatabaseStore,
    kind: SiteDocumentKind,
    locale: String,
) -> Result<Option<PublicRulesDocument>, ApplicationError> {
    let row = sqlx::query(
        "SELECT locale, revision, title, markdown, published_at
         FROM public.rules_documents
         WHERE kind = $1 AND locale = $2",
    )
    .bind(kind.as_str())
    .bind(locale)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 规则文档失败"))?;

    row.map(|row| {
        Ok(PublicRulesDocument {
            locale: row
                .try_get("locale")
                .map_err(|error| storage_error_from(error, "读取规则语言失败"))?,
            revision: row
                .try_get("revision")
                .map_err(|error| storage_error_from(error, "读取规则版本失败"))?,
            title: row
                .try_get("title")
                .map_err(|error| storage_error_from(error, "读取规则标题失败"))?,
            markdown: row
                .try_get("markdown")
                .map_err(|error| storage_error_from(error, "读取规则正文失败"))?,
            published_at: row
                .try_get("published_at")
                .map_err(|error| storage_error_from(error, "读取规则发布时间失败"))?,
        })
    })
    .transpose()
}

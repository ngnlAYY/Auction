//! Public 规则文档查询与 Admin 发布适配器。

#[cfg(feature = "postgres")]
use std::future::Future;

use auction_application::{
    AdminRulesDocument, AdminRulesReader, AdminRulesRequest, ApplicationError, ApplicationFuture,
    PublicRulesDocument, PublicRulesReader, PublicRulesRequest,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminRulesPublishCommand, AdminRulesStoredCommand, AdminRulesTransaction,
    AdminRulesTransactionError, AdminRulesTransactionFailure, AdminRulesTransactionRunner,
    RequestHash, TransactionFuture, TransactionIsolation, TransactionPolicy,
};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Transaction};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use super::PostgresStore;

/// Admin 规则发布使用的 PostgreSQL 事务对象。
#[cfg(feature = "postgres")]
pub struct PostgresRulesTransaction {
    transaction: Option<Transaction<'static, Postgres>>,
}

#[cfg(feature = "postgres")]
impl PostgresRulesTransaction {
    fn new(transaction: Transaction<'static, Postgres>) -> Self {
        Self { transaction: Some(transaction) }
    }

    async fn commit(mut self) -> Result<(), AdminRulesTransactionError> {
        match self.transaction.take() {
            Some(transaction) => transaction.commit().await.map_err(map_sql_error),
            None => Ok(()),
        }
    }

    fn transaction_mut(
        &mut self,
    ) -> Result<&mut Transaction<'static, Postgres>, AdminRulesTransactionError> {
        self.transaction
            .as_mut()
            .ok_or_else(|| AdminRulesTransactionError::Failed("规则事务已经结束".to_owned()))
    }
}

#[cfg(feature = "postgres")]
#[allow(clippy::manual_async_fn)]
impl AdminRulesTransactionRunner for PostgresStore {
    type Transaction = PostgresRulesTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: AdminRulesTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                return Err(E::from(error));
            }
            for attempt in 0..policy.max_attempts {
                let transaction = match self.pool().begin().await {
                    Ok(transaction) => transaction,
                    Err(error) => return Err(E::from(map_sql_error(error))),
                };
                let mut transaction = PostgresRulesTransaction::new(transaction);
                if let Err(error) = set_isolation(&mut transaction, policy.isolation).await {
                    if matches!(error, AdminRulesTransactionError::SerializationConflict)
                        && attempt + 1 < policy.max_attempts
                    {
                        continue;
                    }
                    return Err(E::from(error));
                }
                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(error) => {
                            if matches!(error, AdminRulesTransactionError::SerializationConflict)
                                && attempt + 1 < policy.max_attempts
                            {
                                continue;
                            }
                            return Err(E::from(error));
                        }
                    },
                    Err(error) => {
                        if error.is_serialization_conflict() && attempt + 1 < policy.max_attempts {
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
            Err(E::from(AdminRulesTransactionError::SerializationConflict))
        }
    }
}

#[cfg(feature = "postgres")]
#[allow(clippy::manual_async_fn)]
impl AdminRulesTransaction for PostgresRulesTransaction {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<AdminRulesStoredCommand>, AdminRulesTransactionError>> + Send
    {
        async move {
            let row = sqlx::query(
                "SELECT command_kind, request_digest, locale, result_payload
                 FROM public.rules_admin_commands WHERE command_id = $1",
            )
            .bind(command_id.as_uuid())
            .fetch_optional(&mut **self.transaction_mut()?)
            .await
            .map_err(map_sql_error)?;
            let Some(row) = row else {
                return Ok(None);
            };
            let digest: Vec<u8> = row.try_get("request_digest").map_err(map_sql_error)?;
            let digest: [u8; 32] = digest.try_into().map_err(|_| {
                AdminRulesTransactionError::Failed("规则命令摘要长度无效".to_owned())
            })?;
            let result_payload: Value = row.try_get("result_payload").map_err(map_sql_error)?;
            let command = AdminRulesStoredCommand {
                command_id,
                command_kind: row.try_get("command_kind").map_err(map_sql_error)?,
                locale: row.try_get("locale").map_err(map_sql_error)?,
                request_hash: RequestHash::new(digest),
                result_payload: serde_json::to_vec(&result_payload).map_err(|_| {
                    AdminRulesTransactionError::Failed("规则命令结果编码失败".to_owned())
                })?,
            };
            Ok(Some(command))
        }
    }

    fn insert_command(
        &mut self,
        command: AdminRulesStoredCommand,
    ) -> impl Future<Output = Result<(), AdminRulesTransactionError>> + Send {
        async move {
            let result_payload: Value =
                serde_json::from_slice(&command.result_payload).map_err(|_| {
                    AdminRulesTransactionError::Failed("规则命令结果不是 JSON 对象".to_owned())
                })?;
            sqlx::query(
                "INSERT INTO public.rules_admin_commands
                    (command_id, command_kind, request_digest, locale, result_payload)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(command.command_id.as_uuid())
            .bind(command.command_kind)
            .bind(command.request_hash.as_bytes().to_vec())
            .bind(command.locale)
            .bind(result_payload)
            .execute(&mut **self.transaction_mut()?)
            .await
            .map_err(map_sql_error)?;
            Ok(())
        }
    }

    fn publish_document(
        &mut self,
        command: AdminRulesPublishCommand,
        published_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminRulesDocument, AdminRulesTransactionError>> + Send {
        async move {
            let next_revision = command
                .expected_revision
                .checked_add(1)
                .ok_or_else(|| AdminRulesTransactionError::Failed("规则版本溢出".to_owned()))?;
            let row = sqlx::query(
                "INSERT INTO public.rules_documents
                    (locale, revision, title, markdown, published_by, published_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (locale) DO UPDATE
                 SET revision = $2, title = $3, markdown = $4,
                     published_by = $5, published_at = $6
                 WHERE public.rules_documents.revision = $7
                 RETURNING locale, revision, title, markdown, published_by, published_at",
            )
            .bind(&command.locale)
            .bind(next_revision)
            .bind(&command.title)
            .bind(&command.markdown)
            .bind(&command.actor_id)
            .bind(published_at)
            .bind(command.expected_revision)
            .fetch_optional(&mut **self.transaction_mut()?)
            .await
            .map_err(map_sql_error)?;
            let Some(row) = row else {
                let current_revision = sqlx::query_scalar::<_, i64>(
                    "SELECT revision FROM public.rules_documents WHERE locale = $1",
                )
                .bind(&command.locale)
                .fetch_optional(&mut **self.transaction_mut()?)
                .await
                .map_err(map_sql_error)?
                .unwrap_or(0);
                return Err(AdminRulesTransactionError::RevisionConflict { current_revision });
            };
            Ok(AdminRulesDocument {
                locale: row.try_get("locale").map_err(map_sql_error)?,
                revision: row.try_get("revision").map_err(map_sql_error)?,
                title: row.try_get("title").map_err(map_sql_error)?,
                markdown: row.try_get("markdown").map_err(map_sql_error)?,
                published_by: row.try_get("published_by").map_err(map_sql_error)?,
                published_at: row.try_get("published_at").map_err(map_sql_error)?,
            })
        }
    }

    fn append_publish_record(
        &mut self,
        command: AdminRulesPublishCommand,
        result: AdminRulesDocument,
    ) -> impl Future<Output = Result<(), AdminRulesTransactionError>> + Send {
        async move {
            let metadata = json!({
                "command_id": command.command_id.as_uuid(),
                "locale": result.locale,
                "revision": result.revision,
            });
            sqlx::query(
                "INSERT INTO public.audit_records
                    (id, actor_type, actor_id, action, entity_type, entity_id, result,
                     request_id, source, metadata)
                 VALUES ($1, 'admin', $2, 'rules.publish', 'rules_document', $3,
                         'success', $4, 'admin', $5)",
            )
            .bind(Uuid::new_v4())
            .bind(command.actor_id)
            .bind(result.locale)
            .bind(command.request_id)
            .bind(metadata)
            .execute(&mut **self.transaction_mut()?)
            .await
            .record_audit_failure()
            .map_err(map_sql_error)?;
            Ok(())
        }
    }
}

#[cfg(feature = "postgres")]
async fn set_isolation(
    transaction: &mut PostgresRulesTransaction,
    isolation: TransactionIsolation,
) -> Result<(), AdminRulesTransactionError> {
    let statement = match isolation {
        TransactionIsolation::Serializable => "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        TransactionIsolation::ReadCommitted => "SET TRANSACTION ISOLATION LEVEL READ COMMITTED",
    };
    sqlx::query(statement)
        .execute(&mut **transaction.transaction_mut()?)
        .await
        .map_err(map_sql_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn map_sql_error(error: sqlx::Error) -> AdminRulesTransactionError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            AdminRulesTransactionError::SerializationConflict
        }
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminRulesTransactionError::Conflict
        }
        _ => AdminRulesTransactionError::Failed("PostgreSQL 规则事务操作失败".to_owned()),
    }
}

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

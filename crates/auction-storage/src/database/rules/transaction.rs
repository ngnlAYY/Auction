//! PostgreSQL Admin 规则发布事务与命令账本。

use crate::database::audit_health::AuditWriteResult;
#[cfg(feature = "postgres")]
use crate::database::competition::begin_statement;
use crate::database::{PostgresStore, TrackedTransaction as Transaction};
use auction_application::{
    AdminRulesDocument, AdminRulesPublishCommand, AdminRulesStoredCommand, AdminRulesTransaction,
    AdminRulesTransactionError, AdminRulesTransactionFailure, AdminRulesTransactionRunner,
    RequestHash, TransactionFuture, TransactionPolicy,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Row};
use uuid::Uuid;

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
        self.pool().observe_transaction(
            auction_observability::DatabaseTransactionOwner::Admin,
            async move {
                if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                    return Err(E::from(error));
                }
                for attempt in 0..policy.max_attempts {
                    let transaction =
                        match self.pool().begin_with(begin_statement(policy.isolation)).await {
                            Ok(transaction) => transaction,
                            Err(error) => return Err(E::from(map_sql_error(error))),
                        };
                    let mut transaction = PostgresRulesTransaction::new(transaction);
                    match operation(&mut transaction).await {
                        Ok(value) => match transaction.commit().await {
                            Ok(()) => return Ok(value),
                            Err(error) => {
                                if matches!(
                                    error,
                                    AdminRulesTransactionError::SerializationConflict
                                ) && attempt + 1 < policy.max_attempts
                                {
                                    crate::database::serializable_retry_backoff(usize::from(
                                        attempt + 1,
                                    ))
                                    .await;
                                    continue;
                                }
                                crate::database::observe_retry_exhaustion(
                                    usize::from(attempt + 1),
                                    usize::from(policy.max_attempts),
                                    matches!(
                                        error,
                                        AdminRulesTransactionError::SerializationConflict
                                    ),
                                );
                                return Err(E::from(error));
                            }
                        },
                        Err(error) => {
                            if error.is_serialization_conflict()
                                && attempt + 1 < policy.max_attempts
                            {
                                crate::database::serializable_retry_backoff(usize::from(
                                    attempt + 1,
                                ))
                                .await;
                                continue;
                            }
                            crate::database::observe_retry_exhaustion(
                                usize::from(attempt + 1),
                                usize::from(policy.max_attempts),
                                error.is_serialization_conflict(),
                            );
                            return Err(error);
                        }
                    }
                }
                Err(E::from(AdminRulesTransactionError::SerializationConflict))
            },
        )
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
            .bind(command.request_hash.as_bytes().as_slice())
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
            // 新建只能从 revision 0 开始；更新不得在目标缺失时借 UPSERT 跳过版本 CAS。
            let statement = if command.expected_revision == 0 {
                "INSERT INTO public.rules_documents
                    (locale, revision, title, markdown, published_by, published_at)
                 SELECT $1, $2, $3, $4, $5, $6 WHERE $7::bigint = 0
                 ON CONFLICT (locale) DO NOTHING
                 RETURNING locale, revision, title, markdown, published_by, published_at"
            } else {
                "UPDATE public.rules_documents
                 SET revision = $2, title = $3, markdown = $4,
                     published_by = $5, published_at = $6
                 WHERE locale = $1 AND revision = $7
                 RETURNING locale, revision, title, markdown, published_by, published_at"
            };
            let row = sqlx::query(statement)
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
            let metadata = auction_application::cli_audit_metadata(
                command.operator_identity.as_ref(),
                json!({
                    "command_id": command.command_id.as_uuid(),
                    "locale": result.locale,
                    "revision": result.revision,
                }),
            );
            sqlx::query(
                "INSERT INTO public.audit_records
                    (id, actor_type, actor_id, action, entity_type, entity_id, result,
                     request_id, source, metadata)
                 VALUES ($1, 'admin', $2, 'rules.publish', 'rules_document', $3,
                         'success', $4, $6, $5)",
            )
            .bind(Uuid::new_v4())
            .bind(command.actor_id)
            .bind(result.locale)
            .bind(command.request_id)
            .bind(metadata)
            .bind(if command.operator_identity.is_some() { "cli" } else { "admin" })
            .execute(&mut **self.transaction_mut()?)
            .await
            .record_audit_failure()
            .map_err(map_sql_error)?;
            Ok(())
        }
    }
}

#[cfg(feature = "postgres")]
fn map_sql_error(error: sqlx::Error) -> AdminRulesTransactionError {
    match &error {
        _ if crate::database::observed_serialization_conflict(&error) => {
            AdminRulesTransactionError::SerializationConflict
        }
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminRulesTransactionError::Conflict
        }
        _ => AdminRulesTransactionError::Failed(crate::database::sql_error_context(
            &error,
            "PostgreSQL 规则事务操作失败",
        )),
    }
}

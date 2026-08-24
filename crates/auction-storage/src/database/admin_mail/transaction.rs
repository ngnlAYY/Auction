//! PostgreSQL Admin 邮件命令事务、轮询状态写入和追加审计。

use super::mapping::{map_poll_state, map_sql_error};
use super::*;

pub struct PostgresMailTransaction {
    transaction: Option<Transaction<'static, Postgres>>,
}

impl PostgresMailTransaction {
    fn new(transaction: Transaction<'static, Postgres>) -> Self {
        Self { transaction: Some(transaction) }
    }

    async fn commit(mut self) -> Result<(), AdminMailTransactionError> {
        match self.transaction.take() {
            Some(transaction) => transaction.commit().await.map_err(map_sql_error),
            None => Ok(()),
        }
    }

    fn transaction_mut(
        &mut self,
    ) -> Result<&mut Transaction<'static, Postgres>, AdminMailTransactionError> {
        self.transaction
            .as_mut()
            .ok_or_else(|| AdminMailTransactionError::Failed("邮件 Admin 事务已经结束".to_owned()))
    }
}

#[allow(clippy::manual_async_fn)]
impl AdminMailTransactionRunner for crate::database::PostgresStore {
    type Transaction = PostgresMailTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: AdminMailTransactionFailure,
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
                let mut transaction = PostgresMailTransaction::new(transaction);
                if let Err(error) = set_isolation(&mut transaction, policy.isolation).await {
                    if matches!(error, AdminMailTransactionError::SerializationConflict)
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
                            if matches!(error, AdminMailTransactionError::SerializationConflict)
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
            Err(E::from(AdminMailTransactionError::SerializationConflict))
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl AdminMailTransaction for PostgresMailTransaction {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<AdminMailStoredCommand>, AdminMailTransactionError>> + Send
    {
        async move {
            let row = sqlx::query(
                "SELECT command_kind, request_digest, mailbox_key, result_payload
                 FROM public.mail_admin_commands WHERE command_id = $1",
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
                AdminMailTransactionError::Failed("邮件命令摘要长度无效".to_owned())
            })?;
            let result_payload: Json<Value> =
                row.try_get("result_payload").map_err(map_sql_error)?;
            Ok(Some(AdminMailStoredCommand {
                command_id,
                command_kind: row.try_get("command_kind").map_err(map_sql_error)?,
                mailbox_key: row.try_get("mailbox_key").map_err(map_sql_error)?,
                request_hash: RequestHash::new(digest),
                result_payload: serde_json::to_vec(&result_payload.0).map_err(|_| {
                    AdminMailTransactionError::Failed("邮件命令结果编码失败".to_owned())
                })?,
            }))
        }
    }

    fn insert_command(
        &mut self,
        command: AdminMailStoredCommand,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send {
        async move {
            let result_payload: Value =
                serde_json::from_slice(&command.result_payload).map_err(|_| {
                    AdminMailTransactionError::Failed("邮件命令结果不是 JSON 对象".to_owned())
                })?;
            sqlx::query(
                "INSERT INTO public.mail_admin_commands
                    (command_id, command_kind, request_digest, mailbox_key, result_payload)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(command.command_id.as_uuid())
            .bind(command.command_kind)
            .bind(command.request_hash.as_bytes().to_vec())
            .bind(command.mailbox_key)
            .bind(Json(result_payload))
            .execute(&mut **self.transaction_mut()?)
            .await
            .map_err(map_sql_error)?;
            Ok(())
        }
    }

    fn set_polling_paused(
        &mut self,
        command: AdminMailPollingCommand,
        updated_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminMailPollState, AdminMailTransactionError>> + Send {
        async move {
            let row = sqlx::query(
                "UPDATE public.mail_poll_state
                 SET paused = $1, state_version = state_version + 1, updated_at = $2
                 WHERE mailbox_key = $3 AND state_version = $4
                       AND (claim_expires_at IS NULL OR claim_expires_at <= $2)
                 RETURNING mailbox_key, cursor_value, last_poll_at, next_poll_at,
                           consecutive_failures, paused, state_version, updated_at",
            )
            .bind(command.paused)
            .bind(updated_at)
            .bind(&command.mailbox_key)
            .bind(command.expected_state_version)
            .fetch_optional(&mut **self.transaction_mut()?)
            .await
            .map_err(map_sql_error)?;
            let Some(row) = row else {
                let current = sqlx::query_scalar::<_, i64>(
                    "SELECT state_version FROM public.mail_poll_state WHERE mailbox_key = $1",
                )
                .bind(&command.mailbox_key)
                .fetch_optional(&mut **self.transaction_mut()?)
                .await
                .map_err(map_sql_error)?;
                return match current {
                    Some(current_state_version) => {
                        Err(AdminMailTransactionError::StateConflict { current_state_version })
                    }
                    None => Err(AdminMailTransactionError::NotFound),
                };
            };
            map_poll_state(row)
        }
    }

    fn trigger_poll(
        &mut self,
        command: AdminMailPollTriggerCommand,
        triggered_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminMailPollState, AdminMailTransactionError>> + Send {
        async move {
            let row = if command.expected_state_version == 0 {
                sqlx::query(
                    "INSERT INTO public.mail_poll_state
                        (mailbox_key, cursor_value, last_poll_at, next_poll_at,
                         consecutive_failures, paused, state_version, updated_at)
                     SELECT $1, NULL, $2, $2, 0, false, 1, $2
                     WHERE NOT EXISTS (
                         SELECT 1 FROM public.mail_poll_state WHERE mailbox_key = $1
                     )
                     RETURNING mailbox_key, cursor_value, last_poll_at, next_poll_at,
                               consecutive_failures, paused, state_version, updated_at",
                )
                .bind(&command.mailbox_key)
                .bind(triggered_at)
                .fetch_optional(&mut **self.transaction_mut()?)
                .await
                .map_err(map_sql_error)?
            } else {
                sqlx::query(
                    "UPDATE public.mail_poll_state
                     SET next_poll_at = GREATEST(last_poll_at, $1),
                         state_version = state_version + 1, updated_at = $1
                     WHERE mailbox_key = $2 AND state_version = $3
                       AND (claim_expires_at IS NULL OR claim_expires_at <= $1)
                     RETURNING mailbox_key, cursor_value, last_poll_at, next_poll_at,
                               consecutive_failures, paused, state_version, updated_at",
                )
                .bind(triggered_at)
                .bind(&command.mailbox_key)
                .bind(command.expected_state_version)
                .fetch_optional(&mut **self.transaction_mut()?)
                .await
                .map_err(map_sql_error)?
            };
            let Some(row) = row else {
                let current = sqlx::query_scalar::<_, i64>(
                    "SELECT state_version FROM public.mail_poll_state WHERE mailbox_key = $1",
                )
                .bind(&command.mailbox_key)
                .fetch_optional(&mut **self.transaction_mut()?)
                .await
                .map_err(map_sql_error)?;
                return match current {
                    Some(current_state_version) => {
                        Err(AdminMailTransactionError::StateConflict { current_state_version })
                    }
                    None => Err(AdminMailTransactionError::NotFound),
                };
            };
            map_poll_state(row)
        }
    }

    fn append_pause_record(
        &mut self,
        command: AdminMailPollingCommand,
        result: AdminMailPollState,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send {
        async move {
            let metadata = serde_json::json!({
                "command_id": command.command_id.as_uuid(),
                "mailbox_key": result.mailbox_key,
                "paused": result.paused,
                "state_version": result.state_version,
            });
            sqlx::query(
                "INSERT INTO public.audit_records
                    (id, actor_type, actor_id, action, entity_type, entity_id, result,
                     request_id, source, metadata)
                 VALUES ($1, 'admin', $2, 'mail.set_polling_paused', 'mail_poll_state', $3,
                         'success', $4, 'admin', $5)",
            )
            .bind(Uuid::new_v4())
            .bind(command.actor_id)
            .bind(command.mailbox_key)
            .bind(command.request_id)
            .bind(Json(metadata))
            .execute(&mut **self.transaction_mut()?)
            .await
            .record_audit_failure()
            .map_err(map_sql_error)?;
            Ok(())
        }
    }

    fn append_trigger_record(
        &mut self,
        command: AdminMailPollTriggerCommand,
        result: AdminMailPollState,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send {
        async move {
            let metadata = serde_json::json!({
                "command_id": command.command_id.as_uuid(),
                "mailbox_key": result.mailbox_key,
                "next_poll_at": result.next_poll_at,
                "state_version": result.state_version,
            });
            sqlx::query(
                "INSERT INTO public.audit_records
                    (id, actor_type, actor_id, action, entity_type, entity_id, result,
                     request_id, source, metadata)
                 VALUES ($1, 'admin', $2, 'mail.trigger_poll', 'mail_poll_state', $3,
                         'success', $4, 'admin', $5)",
            )
            .bind(Uuid::new_v4())
            .bind(command.actor_id)
            .bind(command.mailbox_key)
            .bind(command.request_id)
            .bind(Json(metadata))
            .execute(&mut **self.transaction_mut()?)
            .await
            .record_audit_failure()
            .map_err(map_sql_error)?;
            Ok(())
        }
    }
}

async fn set_isolation(
    transaction: &mut PostgresMailTransaction,
    isolation: TransactionIsolation,
) -> Result<(), AdminMailTransactionError> {
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

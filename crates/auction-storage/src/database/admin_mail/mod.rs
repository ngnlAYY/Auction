//! Admin 邮件轮询状态与入站审查的 PostgreSQL 适配器。
//!
//! `inbound_mail` 的正文和内容摘要不跨越存储边界；详情只从 `decision_metadata` 的批准字段
//! 生成固定安全投影。SQLite Lite 没有这些完整档邮件表，因此所有对应能力显式返回 501。

#[cfg(feature = "postgres")]
use std::future::Future;

use auction_application::{
    AdminMailDashboardSnapshot, AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakePage, AdminMailPollState, AdminMailPollStateRequest, AdminMailReader,
    ApplicationError, ApplicationFuture,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminMailIntakeMetadata, AdminMailIntakeSummary, AdminMailPollTriggerCommand,
    AdminMailPollerSnapshot, AdminMailPollingCommand, AdminMailStoredCommand, AdminMailTransaction,
    AdminMailTransactionError, AdminMailTransactionFailure, AdminMailTransactionRunner,
    MAX_ADMIN_MAIL_POLLER_COUNT, RequestHash, TransactionFuture, TransactionIsolation,
    TransactionPolicy,
};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde::Deserialize;
#[cfg(feature = "postgres")]
use serde_json::Value;
#[cfg(feature = "postgres")]
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Transaction, types::Json};
use uuid::Uuid;

use super::DatabaseStore;

impl AdminMailReader for DatabaseStore {
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, AdminMailDashboardSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => dashboard_snapshot_postgres(store).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn poll_state(
        &self,
        request: AdminMailPollStateRequest,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => get_poll_state_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn list_intakes(
        &self,
        request: AdminMailIntakeFilter,
    ) -> ApplicationFuture<'_, AdminMailIntakePage> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_intakes_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn intake_detail(
        &self,
        intake_id: Uuid,
    ) -> ApplicationFuture<'_, Option<AdminMailIntakeDetail>> {
        Box::pin(async move {
            if intake_id.is_nil() {
                return Err(ApplicationError::AdminMail(AdminMailError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => get_intake_postgres(store, intake_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

#[cfg(feature = "postgres")]
pub struct PostgresMailTransaction {
    transaction: Option<Transaction<'static, Postgres>>,
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
#[allow(clippy::manual_async_fn)]
impl AdminMailTransactionRunner for super::PostgresStore {
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

#[cfg(feature = "postgres")]
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
            .map_err(map_sql_error)?;
            Ok(())
        }
    }
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
fn map_sql_error(error: sqlx::Error) -> AdminMailTransactionError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            AdminMailTransactionError::SerializationConflict
        }
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminMailTransactionError::Conflict
        }
        _ => AdminMailTransactionError::Failed("PostgreSQL 邮件 Admin 事务操作失败".to_owned()),
    }
}

#[cfg(feature = "postgres")]
async fn get_poll_state_postgres(
    store: &super::PostgresStore,
    request: AdminMailPollStateRequest,
) -> Result<AdminMailPollState, ApplicationError> {
    let row = sqlx::query(
        "SELECT mailbox_key, cursor_value, last_poll_at, next_poll_at,
                consecutive_failures, paused, state_version, updated_at
         FROM public.mail_poll_state WHERE mailbox_key = $1",
    )
    .bind(request.mailbox_key)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 邮件轮询状态失败"))?;
    let Some(row) = row else {
        return Err(ApplicationError::AdminMailTransaction(AdminMailTransactionError::NotFound));
    };
    let state = map_poll_state(row).map_err(ApplicationError::AdminMailTransaction)?;
    if state.validate() {
        Ok(state)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

#[cfg(feature = "postgres")]
async fn dashboard_snapshot_postgres(
    store: &super::PostgresStore,
) -> Result<AdminMailDashboardSnapshot, ApplicationError> {
    // 轮询器数量是部署配置的一小组固定状态；LIMIT + 1 让损坏或异常膨胀的表不会无限进入
    // Admin 响应，同时仍能明确报告超出安全投影上限的情况。
    let rows = sqlx::query(
        "SELECT mailbox_key, last_poll_at, next_poll_at, consecutive_failures,
                paused, state_version, updated_at
         FROM public.mail_poll_state
         ORDER BY mailbox_key ASC
         LIMIT 65",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 邮件轮询器 dashboard 摘要失败"))?;
    if rows.len() > MAX_ADMIN_MAIL_POLLER_COUNT {
        return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
    }
    let mut pollers = Vec::with_capacity(rows.len());
    for row in &rows {
        let poller = map_mail_poller(row)?;
        if !poller.validate()
            || pollers.iter().any(|previous: &AdminMailPollerSnapshot| {
                previous.mailbox_key == poller.mailbox_key
            })
        {
            return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
        }
        pollers.push(poller);
    }
    let snapshot = AdminMailDashboardSnapshot { observed_at: Utc::now(), pollers };
    if snapshot.validate() {
        Ok(snapshot)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

#[cfg(feature = "postgres")]
async fn list_intakes_postgres(
    store: &super::PostgresStore,
    request: AdminMailIntakeFilter,
) -> Result<AdminMailIntakePage, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启邮件入站读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置邮件入站读取隔离级别失败"))?;

    let mailbox_key = request.mailbox_key.as_deref();
    let world_id = request.world_id.as_deref();
    let kind = request.kind.as_deref();
    let status = request.status.as_deref();
    let decision = request.decision.as_deref();
    let received_from = request.received_from;
    let received_to = request.received_to;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.inbound_mail
         WHERE ($1::text IS NULL OR mailbox_key = $1)
           AND ($2::text IS NULL OR world_id = $2)
           AND ($3::text IS NULL OR mail_kind = $3)
           AND ($4::text IS NULL OR status = $4)
           AND ($5::text IS NULL OR decision_code = $5)
           AND ($6::timestamptz IS NULL OR received_at >= $6)
           AND ($7::timestamptz IS NULL OR received_at <= $7)",
    )
    .bind(mailbox_key)
    .bind(world_id)
    .bind(kind)
    .bind(status)
    .bind(decision)
    .bind(received_from)
    .bind(received_to)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计邮件入站记录失败"))?;
    let rows = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, status,
                decision_code, received_at, processed_at
         FROM public.inbound_mail
         WHERE ($1::text IS NULL OR mailbox_key = $1)
           AND ($2::text IS NULL OR world_id = $2)
           AND ($3::text IS NULL OR mail_kind = $3)
           AND ($4::text IS NULL OR status = $4)
           AND ($5::text IS NULL OR decision_code = $5)
           AND ($6::timestamptz IS NULL OR received_at >= $6)
           AND ($7::timestamptz IS NULL OR received_at <= $7)
         ORDER BY received_at DESC, id DESC
         LIMIT $8 OFFSET $9",
    )
    .bind(mailbox_key)
    .bind(world_id)
    .bind(kind)
    .bind(status)
    .bind(decision)
    .bind(received_from)
    .bind(received_to)
    .bind(request.page_size)
    .bind(request.offset())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取邮件入站列表失败"))?;
    let items =
        rows.into_iter().map(|row| map_intake_summary(&row)).collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await.map_err(|_| storage_error("提交邮件入站读取事务失败"))?;
    let page =
        AdminMailIntakePage { items, total, page: request.page, page_size: request.page_size };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

#[cfg(feature = "postgres")]
async fn get_intake_postgres(
    store: &super::PostgresStore,
    intake_id: Uuid,
) -> Result<Option<AdminMailIntakeDetail>, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, status,
                decision_code, received_at, processed_at, decision_metadata
         FROM public.inbound_mail WHERE id = $1",
    )
    .bind(intake_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 邮件入站详情失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let summary = map_intake_summary(&row)?;
    let decision_metadata: Json<Value> =
        row.try_get("decision_metadata").map_err(|_| storage_error("读取邮件决定元数据失败"))?;
    let metadata = decode_metadata(decision_metadata.0)?;
    if !summary.validate() || !metadata.validate() {
        return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
    }
    Ok(Some(AdminMailIntakeDetail { summary, metadata }))
}

#[cfg(feature = "postgres")]
fn map_poll_state(
    row: sqlx::postgres::PgRow,
) -> Result<AdminMailPollState, AdminMailTransactionError> {
    Ok(AdminMailPollState {
        mailbox_key: row.try_get("mailbox_key").map_err(map_sql_error)?,
        cursor_value: row.try_get("cursor_value").map_err(map_sql_error)?,
        last_poll_at: row.try_get("last_poll_at").map_err(map_sql_error)?,
        next_poll_at: row.try_get("next_poll_at").map_err(map_sql_error)?,
        consecutive_failures: row.try_get("consecutive_failures").map_err(map_sql_error)?,
        paused: row.try_get("paused").map_err(map_sql_error)?,
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
    })
}

#[cfg(feature = "postgres")]
fn map_mail_poller(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminMailPollerSnapshot, ApplicationError> {
    Ok(AdminMailPollerSnapshot {
        mailbox_key: row
            .try_get("mailbox_key")
            .map_err(|_| storage_error("读取邮件轮询器邮箱标识失败"))?,
        last_poll_at: row
            .try_get("last_poll_at")
            .map_err(|_| storage_error("读取邮件轮询器上次轮询时间失败"))?,
        next_poll_at: row
            .try_get("next_poll_at")
            .map_err(|_| storage_error("读取邮件轮询器下次轮询时间失败"))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|_| storage_error("读取邮件轮询器失败计数失败"))?,
        paused: row.try_get("paused").map_err(|_| storage_error("读取邮件轮询器暂停状态失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取邮件轮询器状态版本失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| storage_error("读取邮件轮询器更新时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
fn map_intake_summary(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminMailIntakeSummary, ApplicationError> {
    let summary = AdminMailIntakeSummary {
        id: row.try_get("id").map_err(|_| storage_error("读取邮件入站标识失败"))?,
        mailbox_key: row
            .try_get("mailbox_key")
            .map_err(|_| storage_error("读取邮件邮箱标识失败"))?,
        external_mail_id: row
            .try_get("external_mail_id")
            .map_err(|_| storage_error("读取外部邮件标识失败"))?,
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取邮件世界标识失败"))?,
        kind: row.try_get("mail_kind").map_err(|_| storage_error("读取邮件类型失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取邮件状态失败"))?,
        decision_code: row
            .try_get("decision_code")
            .map_err(|_| storage_error("读取邮件决定码失败"))?,
        received_at: row
            .try_get("received_at")
            .map_err(|_| storage_error("读取邮件接收时间失败"))?,
        processed_at: row
            .try_get("processed_at")
            .map_err(|_| storage_error("读取邮件处理时间失败"))?,
    };
    if summary.validate() {
        Ok(summary)
    } else {
        Err(storage_error("邮件入站摘要安全投影无效"))
    }
}

#[cfg(feature = "postgres")]
#[derive(Debug, Default, Deserialize)]
struct DecisionMetadata {
    auction_id: Option<String>,
    submission_id: Option<String>,
    user_id: Option<String>,
    added_count: Option<i64>,
    existing_count: Option<i64>,
    preview_revision: Option<i64>,
    attachment_count: Option<i64>,
    owner_result_code: Option<String>,
    reason_code: Option<String>,
    warning_codes: Option<Vec<String>>,
    delivery_ids: Option<Vec<String>>,
}

#[cfg(feature = "postgres")]
fn decode_metadata(value: Value) -> Result<AdminMailIntakeMetadata, ApplicationError> {
    let value = serde_json::from_value::<DecisionMetadata>(value)
        .map_err(|_| storage_error("邮件决定元数据格式无效"))?;
    let added_count = to_i32(value.added_count.unwrap_or_default())?;
    let existing_count = to_i32(value.existing_count.unwrap_or_default())?;
    let attachment_count = to_i32(value.attachment_count.unwrap_or_default())?;
    let metadata = AdminMailIntakeMetadata {
        auction_id: value.auction_id.unwrap_or_default(),
        submission_id: value.submission_id.unwrap_or_default(),
        user_id: value.user_id.unwrap_or_default(),
        added_count,
        existing_count,
        preview_revision: value.preview_revision.unwrap_or_default(),
        attachment_count,
        owner_result_code: value.owner_result_code.unwrap_or_default(),
        reason_code: value.reason_code.unwrap_or_default(),
        warning_codes: value.warning_codes.unwrap_or_default(),
        delivery_ids: value.delivery_ids.unwrap_or_default(),
    };
    Ok(metadata)
}

#[cfg(feature = "postgres")]
fn to_i32(value: i64) -> Result<i32, ApplicationError> {
    i32::try_from(value).map_err(|_| storage_error("邮件决定计数超出安全范围"))
}

#[cfg(feature = "sqlite")]
fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("Admin 邮件状态与入站审查仅支持 PostgreSQL 完整档")
}

#[cfg(feature = "postgres")]
fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}

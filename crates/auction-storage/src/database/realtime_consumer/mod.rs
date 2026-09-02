//! Worker 实时事件消费的持久化适配器。
//!
//! 本模块只负责应用端口校验和后端分派；stream 目录、事件读取、游标确认及 failure
//! 生命周期分别由同目录的具名模块持有。游标只按 stream 的下一个连续序号推进，失败
//! 记录不改变游标，避免把运维恢复路径混入普通消费入口。

use auction_application::{
    ApplicationFuture, RealtimeConsumerBatch, RealtimeConsumerRequest, RealtimeConsumerStore,
    RealtimeStreamStore,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DatabaseStore;

mod acknowledgement;
mod common;
mod failure;
mod ownership;
mod read;
mod stream;

pub use ownership::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};

#[cfg(feature = "postgres")]
use acknowledgement::acknowledge_postgres;
#[cfg(feature = "sqlite")]
use acknowledgement::acknowledge_sqlite;
use common::{storage_error, validate_cursor_keys, validate_event_position, validate_failure_text};
#[cfg(feature = "postgres")]
use failure::{record_failure_postgres, resolve_failure_postgres};
#[cfg(feature = "sqlite")]
use failure::{record_failure_sqlite, resolve_failure_sqlite};
#[cfg(feature = "postgres")]
use read::read_postgres;
#[cfg(feature = "sqlite")]
use read::read_sqlite;
#[cfg(feature = "postgres")]
use stream::list_streams_postgres;
#[cfg(feature = "sqlite")]
use stream::list_streams_sqlite;

impl RealtimeStreamStore for DatabaseStore {
    fn list_stream_keys(
        &self,
        prefix: String,
        after: String,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<String>> {
        Box::pin(async move {
            stream::validate_stream_directory_query(&prefix, &after, limit)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    list_streams_postgres(store, &prefix, &after, limit).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    list_streams_sqlite(store, &prefix, &after, limit).await
                }
            }
        })
    }
}

impl RealtimeConsumerStore for DatabaseStore {
    fn read_batch(
        &self,
        request: RealtimeConsumerRequest,
    ) -> ApplicationFuture<'_, Option<RealtimeConsumerBatch>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => read_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => read_sqlite(store, request).await,
            }
        })
    }

    fn acknowledge(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        stream_sequence: i64,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            validate_cursor_keys(&consumer_key, &stream_key)?;
            validate_event_position(event_id, stream_sequence)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    acknowledge_postgres(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        stream_sequence,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    acknowledge_sqlite(store, &consumer_key, &stream_key, event_id, stream_sequence)
                        .await
                }
            }
        })
    }

    fn record_failure(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        failure_code: String,
        safe_detail: String,
        failed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            validate_cursor_keys(&consumer_key, &stream_key)?;
            validate_event_position(event_id, 1)?;
            validate_failure_text(&failure_code, &safe_detail)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    record_failure_postgres(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        &failure_code,
                        &safe_detail,
                        failed_at,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    record_failure_sqlite(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        &failure_code,
                        &safe_detail,
                        failed_at,
                    )
                    .await
                }
            }
        })
    }

    fn resolve_failure_after_replay(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        replay_event_id: Uuid,
        resolved_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            validate_cursor_keys(&consumer_key, &stream_key)?;
            validate_event_position(event_id, 1)?;
            validate_event_position(replay_event_id, 1)?;
            if event_id == replay_event_id {
                return Err(storage_error("实时事件不能引用自身作为重放事件"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    resolve_failure_postgres(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        replay_event_id,
                        resolved_at,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    resolve_failure_sqlite(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        replay_event_id,
                        resolved_at,
                    )
                    .await
                }
            }
        })
    }
}

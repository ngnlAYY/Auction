//! Worker 实时事件消费的持久化适配器；本模块只做应用端口校验和后端分派，stream、事件读取、
//! 游标确认与 failure 生命周期由具名子模块持有。游标只按 stream 连续序号推进，失败记录不改它。

use auction_application::{
    ApplicationFuture, RealtimeConsumerBatch, RealtimeConsumerRequest, RealtimeConsumerStore,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DatabaseStore;

mod acknowledgement;
mod common;
mod failure;
mod owned;
mod ownership;
mod read;
mod stream;

pub use ownership::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};

#[cfg(feature = "postgres")]
use acknowledgement::acknowledge_postgres;
use common::{storage_error, validate_cursor_keys, validate_event_position, validate_failure_text};
#[cfg(feature = "postgres")]
pub(super) use failure::resolve_failure_postgres_tx;
#[cfg(feature = "postgres")]
use failure::{record_failure_postgres, resolve_failure_postgres};
#[cfg(feature = "postgres")]
use read::read_postgres;
#[cfg(feature = "postgres")]
use stream::list_pending_streams_postgres;

// stream 目录只由持锁 `RealtimeConsumerOwnership` 实现；连接池入口不提供目录扫描，避免 relay 绕过互斥 session。
impl RealtimeConsumerStore for DatabaseStore {
    fn read_batch(
        &self,
        request: RealtimeConsumerRequest,
    ) -> ApplicationFuture<'_, Option<RealtimeConsumerBatch>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let mut connection = store
                        .pool()
                        .acquire()
                        .await
                        .map_err(|_| storage_error("打开 PostgreSQL 实时消费连接失败"))?;
                    read_postgres(&mut connection, request).await
                }
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
                    let mut connection = store
                        .pool()
                        .acquire()
                        .await
                        .map_err(|_| storage_error("打开 PostgreSQL 实时游标连接失败"))?;
                    acknowledge_postgres(
                        &mut connection,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        stream_sequence,
                    )
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
                    let mut connection = store
                        .pool()
                        .acquire()
                        .await
                        .map_err(|_| storage_error("打开 PostgreSQL 实时失败记录连接失败"))?;
                    record_failure_postgres(
                        &mut connection,
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
                    let mut connection = store
                        .pool()
                        .acquire()
                        .await
                        .map_err(|_| storage_error("打开 PostgreSQL 实时重放连接失败"))?;
                    resolve_failure_postgres(
                        &mut connection,
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

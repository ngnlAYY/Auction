//! Worker 实时事件消费的持久化适配器；本模块只做应用端口校验和后端分派，stream、事件读取、
//! 游标确认与 failure 生命周期由具名子模块持有。游标只按 stream 连续序号推进，失败记录不改它。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::auction::ApplicationFuture;
use auction_application::realtime::{
    RealtimeConsumerBatch, RealtimeConsumerRequest, RealtimeConsumerStore,
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
use failure::record_failure_postgres;
#[cfg(feature = "postgres")]
pub(super) use failure::resolve_failure_postgres_tx;
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
            {
                let mut connection = self.pool().acquire().await.map_err(|error| {
                    storage_error_from(error, "打开 PostgreSQL 实时消费连接失败")
                })?;
                read_postgres(&mut connection, request).await
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
            {
                let mut connection = self.pool().acquire().await.map_err(|error| {
                    storage_error_from(error, "打开 PostgreSQL 实时游标连接失败")
                })?;
                acknowledge_postgres(
                    &mut connection,
                    &consumer_key,
                    &stream_key,
                    event_id,
                    stream_sequence,
                )
                .await
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
            {
                let mut connection = self.pool().acquire().await.map_err(|error| {
                    storage_error_from(error, "打开 PostgreSQL 实时失败记录连接失败")
                })?;
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
        })
    }
}

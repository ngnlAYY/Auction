//! Public relay 的独占消费端口；任何 PostgreSQL 操作都不能绕过持锁 session。

use auction_application::{
    ApplicationError, ApplicationFuture, RealtimeConsumerBatch, RealtimeConsumerRequest,
    RealtimeConsumerStore, RealtimeStreamStore,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::common::{
    storage_error, validate_cursor_keys, validate_event_position, validate_failure_text,
};
use super::ownership::{OwnedRealtimeBackend, RealtimeConsumerOwnership};
use super::stream::validate_stream_directory_query;

impl RealtimeConsumerOwnership {
    fn validate_owner(&self, consumer_key: &str) -> Result<(), ApplicationError> {
        if consumer_key != self.consumer_key {
            return Err(storage_error("实时消费请求与持锁 consumer key 不一致"));
        }
        Ok(())
    }
}

impl RealtimeStreamStore for RealtimeConsumerOwnership {
    fn list_stream_keys(
        &self,
        prefix: String,
        after: String,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<String>> {
        Box::pin(async move {
            validate_stream_directory_query(&prefix, &after, limit)?;
            match &self.backend {
                #[cfg(feature = "postgres")]
                OwnedRealtimeBackend::Postgres(connection) => {
                    // 空目录扫描也使用原 session，不能在锁已失效时继续报告 relay 健康。
                    let mut connection = connection.lock().await;
                    super::list_streams_postgres(&mut connection, &prefix, &after, limit).await
                }
            }
        })
    }
}

impl RealtimeConsumerStore for RealtimeConsumerOwnership {
    fn read_batch(
        &self,
        request: RealtimeConsumerRequest,
    ) -> ApplicationFuture<'_, Option<RealtimeConsumerBatch>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            self.validate_owner(&request.consumer_key)?;
            match &self.backend {
                #[cfg(feature = "postgres")]
                OwnedRealtimeBackend::Postgres(connection) => {
                    let mut connection = connection.lock().await;
                    super::read_postgres(&mut connection, request).await
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
            self.validate_owner(&consumer_key)?;
            validate_event_position(event_id, stream_sequence)?;
            match &self.backend {
                #[cfg(feature = "postgres")]
                OwnedRealtimeBackend::Postgres(connection) => {
                    // Mutex 持有至 commit/rollback 完成；连接断开后不得从池另取 session 确认。
                    let mut connection = connection.lock().await;
                    super::acknowledge_postgres(
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
            self.validate_owner(&consumer_key)?;
            validate_event_position(event_id, 1)?;
            validate_failure_text(&failure_code, &safe_detail)?;
            match &self.backend {
                #[cfg(feature = "postgres")]
                OwnedRealtimeBackend::Postgres(connection) => {
                    let mut connection = connection.lock().await;
                    super::record_failure_postgres(
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
            self.validate_owner(&consumer_key)?;
            validate_event_position(event_id, 1)?;
            validate_event_position(replay_event_id, 1)?;
            if event_id == replay_event_id {
                return Err(storage_error("实时事件不能引用自身作为重放事件"));
            }
            match &self.backend {
                #[cfg(feature = "postgres")]
                OwnedRealtimeBackend::Postgres(connection) => {
                    let mut connection = connection.lock().await;
                    super::resolve_failure_postgres(
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

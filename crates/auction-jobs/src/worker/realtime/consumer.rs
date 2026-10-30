//! 持久化实时事件消费编排；只做读取 → 外部 sink → 连续确认的 at-least-once 管线，
//! 不改变拍卖状态，未注入 sink 时不启动。
//! 内存广播器不是事件事实。

use std::{future::Future, pin::Pin, sync::Arc};

use auction_application::{
    ApplicationError, Clock, RealtimeConsumerRequest, RealtimeConsumerStore, RealtimeEventSink,
};
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use super::{PersistentWorker, poll_delay};

#[derive(Debug, Error)]
pub enum RealtimeConsumerConfigError {
    #[error("{0}")]
    Invalid(&'static str),
}

/// 可插入持久化 Worker 的实时消费 runner。
pub trait RealtimeConsumerRunner: Send + Sync {
    fn run_once(
        &self,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<usize, ApplicationError>> + Send + '_>>;
}

/// 使用一个持久化 consumer/stream 游标向外部 sink 可靠转发事件。
pub struct PersistentRealtimeConsumer {
    store: Arc<dyn RealtimeConsumerStore>,
    sink: Arc<dyn RealtimeEventSink>,
    request: RealtimeConsumerRequest,
}

impl PersistentRealtimeConsumer {
    /// 建立一个有界、按 stream 顺序工作的实时消费 runner。
    pub fn new(
        store: Arc<dyn RealtimeConsumerStore>,
        sink: Arc<dyn RealtimeEventSink>,
        consumer_key: String,
        stream_key: String,
        batch_size: u16,
    ) -> Result<Self, RealtimeConsumerConfigError> {
        let request = RealtimeConsumerRequest {
            consumer_key,
            stream_key,
            batch_size,
            allow_discovery_stream: false,
        };
        request.validate().map_err(RealtimeConsumerConfigError::Invalid)?;
        Ok(Self { store, sink, request })
    }

    async fn run_once_inner(&self, now: DateTime<Utc>) -> Result<usize, ApplicationError> {
        let Some(batch) = self.store.read_batch(self.request.clone()).await? else {
            return Ok(0);
        };
        if batch.consumer_key != self.request.consumer_key
            || batch.stream_key != self.request.stream_key
        {
            return Err(repository_error("实时消费存储返回了不匹配的游标范围"));
        }
        let Some(first_event) = batch.events.first() else {
            return Ok(0);
        };
        if batch.sequence_gap {
            self.record_failure(
                first_event.event_id,
                "sequence_gap",
                "实时 stream 序号存在缺口，等待显式恢复或重放",
                now,
            )
            .await?;
            return Ok(0);
        }

        let mut expected_sequence =
            batch.after.checked_add(1).ok_or_else(|| repository_error("实时消费游标溢出"))?;
        let mut processed = 0_usize;
        for event in batch.events {
            if event.stream_sequence != expected_sequence {
                self.record_failure(
                    event.event_id,
                    "sequence_gap",
                    "实时 stream 事件不是游标要求的连续序号",
                    now,
                )
                .await?;
                break;
            }
            if event.stream_key != self.request.stream_key {
                self.record_failure(
                    event.event_id,
                    "stream_mismatch",
                    "实时事件 stream 与消费配置不一致",
                    now,
                )
                .await?;
                break;
            }
            if let Err(detail) = event.validate() {
                self.record_failure(event.event_id, "schema_invalid", detail, now).await?;
                break;
            }

            // sink 消费完整事件；确认与失败记录只保留两个 Copy 标识，不复制 JSON payload。
            let event_id = event.event_id;
            let stream_sequence = event.stream_sequence;
            match self.sink.publish(event).await {
                Ok(()) => {
                    if let Err(error) = self
                        .store
                        .acknowledge(
                            self.request.consumer_key.clone(),
                            self.request.stream_key.clone(),
                            event_id,
                            stream_sequence,
                        )
                        .await
                    {
                        // sink 可能已经收到事件；记录失败而不是重新发送一个没有持久化边界的
                        // 新事件。下次重试仍使用 event_id 幂等投递。
                        self.record_failure(
                            event_id,
                            "cursor_advance_failed",
                            "实时事件已交给 sink，但消费游标提交失败",
                            now,
                        )
                        .await?;
                        return Err(error);
                    }
                    processed += 1;
                    expected_sequence = expected_sequence
                        .checked_add(1)
                        .ok_or_else(|| repository_error("实时消费事件序号溢出"))?;
                }
                Err(error) => {
                    let (failure_code, safe_detail) =
                        safe_sink_error(&error.failure_code, &error.safe_detail);
                    self.record_failure(event_id, &failure_code, &safe_detail, now).await?;
                    break;
                }
            }
        }
        Ok(processed)
    }

    async fn record_failure(
        &self,
        event_id: Uuid,
        failure_code: &str,
        safe_detail: &str,
        failed_at: DateTime<Utc>,
    ) -> Result<(), ApplicationError> {
        self.store
            .record_failure(
                self.request.consumer_key.clone(),
                self.request.stream_key.clone(),
                event_id,
                failure_code.to_owned(),
                safe_detail.to_owned(),
                failed_at,
            )
            .await
    }
}

impl RealtimeConsumerRunner for PersistentRealtimeConsumer {
    fn run_once(
        &self,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<usize, ApplicationError>> + Send + '_>> {
        Box::pin(self.run_once_inner(now))
    }
}

/// 独立运行可选的外部实时 sink；单轮失败只影响该消费游标，不终止其它 Worker owner。
pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        if worker.control.is_draining() {
            return;
        }
        let started = std::time::Instant::now();
        let now = worker.clock.now();
        let result = if let Some(consumer) = &worker.realtime_consumer {
            match consumer.run_once(now).await {
                Ok(processed) if processed > 0 => {
                    tracing::info!(processed, "realtime consumer advanced persisted cursor");
                    WorkerResult::Success
                }
                Ok(_) => WorkerResult::Success,
                Err(error) => {
                    tracing::warn!(error = %error, "realtime consumer cycle failed; retrying");
                    WorkerResult::Error
                }
            }
        } else {
            WorkerResult::Success
        };
        worker.metrics.observe_iteration(WorkerLoop::Realtime, result, started.elapsed());
        let wait = poll_delay(worker.auction.poll_interval, worker.auction.jitter);
        let deadline = tokio::time::Instant::now() + wait;
        if super::wait_until_or_shutdown(&mut shutdown, deadline).await {
            return;
        }
        worker.metrics.observe_schedule_delay(
            WorkerLoop::Realtime,
            tokio::time::Instant::now().saturating_duration_since(deadline),
        );
    }
}

fn safe_sink_error(failure_code: &str, safe_detail: &str) -> (String, String) {
    let valid_code = auction_domain::valid_text(failure_code, 128);
    let valid_detail = auction_domain::valid_text(safe_detail, 2_048);
    if valid_code && valid_detail {
        (failure_code.to_owned(), safe_detail.to_owned())
    } else {
        ("sink_failed".to_owned(), "外部实时 sink 返回了无效的失败分类".to_owned())
    }
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

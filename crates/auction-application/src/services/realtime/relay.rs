//! Public 进程的持久化 realtime relay 编排。
//! relay 只负责按 stream 读取已提交事件、交给进程内 Hub sink 并连续确认游标。它不改变
//! 拍卖状态，也不把进程内广播误报成外部广播系统已经完成；外部 sink 仍由独立适配器注入。

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::{
    ApplicationError, RealtimeConsumerRequest, RealtimeConsumerStore, RealtimeEventSink,
    RealtimeStreamStore, RepositoryError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeRelayConfig {
    /// durable consumer 的稳定标识；同一 Public 实例重启必须保持不变。
    pub consumer_key: String,
    /// 只扫描拍卖事件 stream；当前固定为 `auction:`。
    pub stream_prefix: String,
    /// 单轮最多读取的 stream 数量。
    pub stream_batch_size: u16,
    /// 单个 stream 单轮最多转发的事件数量。
    pub event_batch_size: u16,
}

impl RealtimeRelayConfig {
    /// 校验 relay 的稳定命名和批次边界。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.consumer_key.trim().is_empty()
            || self.consumer_key != self.consumer_key.trim()
            || self.consumer_key.len() > 128
            || self.consumer_key.chars().any(char::is_control)
            || self.stream_prefix != "auction:"
            || self.stream_batch_size == 0
            || self.stream_batch_size > 200
            || self.event_batch_size == 0
            || self.event_batch_size > 200
        {
            return Err("实时 relay 配置无效");
        }
        Ok(())
    }
}

/// 单次 relay 扫描的安全摘要。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RealtimeRelayBatch {
    pub streams: usize,
    pub delivered: usize,
    pub failures: usize,
}

/// 从持久化 stream 目录追赶并交给进程内广播 sink 的 relay。
pub struct RealtimeRelay {
    stream_store: Arc<dyn RealtimeStreamStore>,
    consumer_store: Arc<dyn RealtimeConsumerStore>,
    sink: Arc<dyn RealtimeEventSink>,
    config: RealtimeRelayConfig,
}

impl RealtimeRelay {
    /// 建立不启动任务的 relay；轮询节奏由 Public 装配根控制。
    pub fn new(
        stream_store: Arc<dyn RealtimeStreamStore>,
        consumer_store: Arc<dyn RealtimeConsumerStore>,
        sink: Arc<dyn RealtimeEventSink>,
        config: RealtimeRelayConfig,
    ) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self { stream_store, consumer_store, sink, config })
    }

    /// 运行一轮有界扫描；失败事件会留在当前游标前等待重试，不会跳过历史。
    pub fn run_once(&self, now: DateTime<Utc>) -> crate::ApplicationFuture<'_, RealtimeRelayBatch> {
        Box::pin(async move {
            if now.timestamp_micros() == 0 {
                return Err(repository_error("实时 relay 时间无效"));
            }
            let mut result = RealtimeRelayBatch::default();
            let mut after_stream = String::new();
            loop {
                let streams = self
                    .stream_store
                    .list_stream_keys(
                        self.config.stream_prefix.clone(),
                        after_stream.clone(),
                        self.config.stream_batch_size,
                    )
                    .await?;
                validate_stream_page(&streams, &self.config.stream_prefix, &after_stream)?;
                if streams.is_empty() {
                    break;
                }
                for stream_key in &streams {
                    result.streams = result.streams.saturating_add(1);
                    let (delivered, failures) = self.consume_stream(stream_key, now).await?;
                    result.delivered = result.delivered.saturating_add(delivered);
                    result.failures = result.failures.saturating_add(failures);
                }
                if streams.len() < usize::from(self.config.stream_batch_size) {
                    break;
                }
                after_stream = streams
                    .last()
                    .cloned()
                    .ok_or_else(|| repository_error("实时 relay stream 页为空"))?;
            }
            Ok(result)
        })
    }

    async fn consume_stream(
        &self,
        stream_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(usize, usize), ApplicationError> {
        let request = RealtimeConsumerRequest {
            consumer_key: self.config.consumer_key.clone(),
            stream_key: stream_key.to_owned(),
            batch_size: self.config.event_batch_size,
            allow_discovery_stream: true,
        };
        let Some(batch) = self.consumer_store.read_batch(request.clone()).await? else {
            return Ok((0, 0));
        };
        if batch.consumer_key != request.consumer_key || batch.stream_key != request.stream_key {
            return Err(repository_error("实时 relay 存储返回了不匹配的 stream"));
        }
        if batch.events.len() > usize::from(request.batch_size) {
            return Err(repository_error("实时 relay 存储返回了超出上限的事件页"));
        }
        let Some(first_event) = batch.events.first() else {
            return Ok((0, 0));
        };
        if batch.sequence_gap {
            self.record_failure(
                &request,
                first_event.event_id,
                "sequence_gap",
                "实时 relay stream 序号存在缺口，等待显式恢复或重放",
                now,
            )
            .await?;
            return Ok((0, 1));
        }

        let mut expected_sequence = batch
            .after
            .checked_add(1)
            .ok_or_else(|| repository_error("实时 relay 消费游标溢出"))?;
        let mut delivered = 0_usize;
        for event in batch.events {
            if event.stream_sequence != expected_sequence {
                self.record_failure(
                    &request,
                    event.event_id,
                    "sequence_gap",
                    "实时 relay 事件不是游标要求的连续序号",
                    now,
                )
                .await?;
                return Ok((delivered, 1));
            }
            if event.stream_key != request.stream_key {
                self.record_failure(
                    &request,
                    event.event_id,
                    "stream_mismatch",
                    "实时 relay 事件 stream 与消费配置不一致",
                    now,
                )
                .await?;
                return Ok((delivered, 1));
            }
            if let Err(detail) = event.validate() {
                self.record_failure(&request, event.event_id, "schema_invalid", detail, now)
                    .await?;
                return Ok((delivered, 1));
            }
            if let Err(error) = self.sink.publish(event.clone()).await {
                let (code, detail) = safe_sink_error(&error.failure_code, &error.safe_detail);
                self.record_failure(&request, event.event_id, &code, &detail, now).await?;
                return Ok((delivered, 1));
            }
            if let Err(error) = self
                .consumer_store
                .acknowledge(
                    request.consumer_key.clone(),
                    request.stream_key.clone(),
                    event.event_id,
                    event.stream_sequence,
                )
                .await
            {
                self.record_failure(
                    &request,
                    event.event_id,
                    "cursor_advance_failed",
                    "实时 relay 已交给本地 sink，但消费游标提交失败",
                    now,
                )
                .await?;
                return Err(error);
            }
            delivered = delivered.saturating_add(1);
            expected_sequence = expected_sequence
                .checked_add(1)
                .ok_or_else(|| repository_error("实时 relay 事件序号溢出"))?;
        }
        Ok((delivered, 0))
    }

    async fn record_failure(
        &self,
        request: &RealtimeConsumerRequest,
        event_id: uuid::Uuid,
        failure_code: &str,
        safe_detail: &str,
        failed_at: DateTime<Utc>,
    ) -> Result<(), ApplicationError> {
        self.consumer_store
            .record_failure(
                request.consumer_key.clone(),
                request.stream_key.clone(),
                event_id,
                failure_code.to_owned(),
                safe_detail.to_owned(),
                failed_at,
            )
            .await
    }
}

fn validate_stream_page(
    streams: &[String],
    prefix: &str,
    after: &str,
) -> Result<(), ApplicationError> {
    let mut previous = after;
    for stream in streams {
        if stream.len() > 255
            || stream.trim().is_empty()
            || stream != stream.trim()
            || stream.chars().any(char::is_control)
            || !stream.starts_with(prefix)
            || stream.as_str() <= previous
        {
            return Err(repository_error("实时 relay stream 目录顺序或命名无效"));
        }
        previous = stream;
    }
    Ok(())
}

fn safe_sink_error(failure_code: &str, safe_detail: &str) -> (String, String) {
    let valid_code = !failure_code.trim().is_empty()
        && failure_code == failure_code.trim()
        && failure_code.len() <= 128
        && !failure_code.chars().any(char::is_control);
    let valid_detail = !safe_detail.trim().is_empty()
        && safe_detail == safe_detail.trim()
        && safe_detail.len() <= 2_048
        && !safe_detail.chars().any(char::is_control);
    if valid_code && valid_detail {
        (failure_code.to_owned(), safe_detail.to_owned())
    } else {
        ("sink_failed".to_owned(), "实时 relay sink 返回了无效的失败分类".to_owned())
    }
}

fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}

//! relay 兜底喉管的编排测试：补号必须发生在目录扫描与推送之前，且补号失败不得让
//! relay 轮次失败（否则会把 Public 就绪翻成不可用）。
//!
//! `auction-application` 不依赖异步运行时，因此这组测试放在已使用 Tokio 的 jobs crate；
#![allow(clippy::expect_used)]
//! 被测对象仍是 `auction_application::realtime::RealtimeRelay` 的公开编排。

use std::sync::{Arc, Mutex};

use auction_application::auction::ApplicationFuture;
use auction_application::core::RepositoryError;
use auction_application::realtime::{
    PublicRealtimeEvent, RealtimeConsumerBatch, RealtimeConsumerRequest, RealtimeConsumerStore,
    RealtimeEventSink, RealtimeRelay, RealtimeRelayConfig, RealtimeSequenceAllocator,
    RealtimeSinkError, RealtimeStreamStore,
};
use chrono::{TimeZone, Utc};

/// 记录调用顺序的共享日志；只用于断言编排顺序，不参与业务判定。
#[derive(Default)]
struct Trace(Mutex<Vec<&'static str>>);

impl Trace {
    fn push(&self, entry: &'static str) {
        if let Ok(mut log) = self.0.lock() {
            log.push(entry);
        }
    }

    fn entries(&self) -> Vec<&'static str> {
        self.0.lock().map(|log| log.clone()).unwrap_or_default()
    }
}

struct Streams {
    trace: Arc<Trace>,
    streams: Vec<String>,
}

impl RealtimeStreamStore for Streams {
    fn list_pending_stream_keys(
        &self,
        _consumer_key: String,
        _prefix: String,
        _after: String,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<String>> {
        self.trace.push("scan");
        let streams = self.streams.clone();
        Box::pin(async move { Ok(streams) })
    }
}

struct Consumers;

impl RealtimeConsumerStore for Consumers {
    fn read_batch(
        &self,
        request: RealtimeConsumerRequest,
    ) -> ApplicationFuture<'_, Option<RealtimeConsumerBatch>> {
        Box::pin(async move {
            Ok(Some(RealtimeConsumerBatch {
                consumer_key: request.consumer_key,
                stream_key: request.stream_key,
                after: 0,
                events: vec![PublicRealtimeEvent {
                    event_id: uuid::Uuid::parse_str("1f0c0000-0000-4000-8000-000000000001")
                        .expect("固定事件标识应为合法 UUID"),
                    stream_key: "auction:1f0c0000-0000-4000-8000-000000000002".to_owned(),
                    stream_sequence: 1,
                    kind: "auction.opened".to_owned(),
                    schema_version: 1,
                    payload: serde_json::json!({
                        "auction_id": "1f0c0000-0000-4000-8000-000000000002",
                        "world_id": "main",
                        "state_version": 1,
                        "preview_revision": 1,
                    }),
                    created_at: Utc
                        .with_ymd_and_hms(2026, 11, 4, 0, 0, 0)
                        .single()
                        .expect("固定时间为合法时刻"),
                }],
                sequence_gap: false,
            }))
        })
    }

    fn acknowledge(
        &self,
        _consumer_key: String,
        _stream_key: String,
        _event_id: uuid::Uuid,
        _stream_sequence: i64,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { Ok(()) })
    }

    fn record_failure(
        &self,
        _consumer_key: String,
        _stream_key: String,
        _event_id: uuid::Uuid,
        _failure_code: String,
        _safe_detail: String,
        _failed_at: chrono::DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { Ok(()) })
    }
}

struct Sink {
    trace: Arc<Trace>,
}

impl RealtimeEventSink for Sink {
    fn publish(
        &self,
        _event: PublicRealtimeEvent,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), RealtimeSinkError>> + Send + '_>> {
        self.trace.push("publish");
        Box::pin(async move { Ok(()) })
    }
}

struct Allocator {
    trace: Arc<Trace>,
    fail: bool,
    allocated: u64,
}

impl RealtimeSequenceAllocator for Allocator {
    fn allocate_pending_sequences(&self, prefix: String) -> ApplicationFuture<'_, u64> {
        self.trace.push("allocate");
        assert_eq!(prefix, "auction:", "relay 只补齐拍卖流前缀");
        let fail = self.fail;
        let allocated = self.allocated;
        Box::pin(async move {
            if fail {
                return Err(RepositoryError::Failed("分配器不可用".to_owned()).into());
            }
            Ok(allocated)
        })
    }
}

fn relay(trace: Arc<Trace>, fail: bool, streams: Vec<String>) -> RealtimeRelay {
    RealtimeRelay::new(
        Arc::new(Streams { trace: Arc::clone(&trace), streams }),
        Arc::new(Consumers),
        Arc::new(Sink { trace: Arc::clone(&trace) }),
        Arc::new(Allocator { trace, fail, allocated: 2 }),
        RealtimeRelayConfig {
            consumer_key: "public-1".to_owned(),
            stream_prefix: "auction:".to_owned(),
            stream_batch_size: 10,
            event_batch_size: 10,
        },
    )
    .expect("relay 配置有效")
}

#[tokio::test]
async fn relay_allocates_pending_sequences_before_scanning() {
    let trace = Arc::new(Trace::default());
    let relay = relay(
        Arc::clone(&trace),
        false,
        vec!["auction:1f0c0000-0000-4000-8000-000000000002".to_owned()],
    );
    let batch = relay.run_once(Utc::now()).await.expect("relay 轮次成功");
    assert_eq!(batch.streams, 1, "relay 应扫描到一个待转发 stream：{batch:?}");
    assert_eq!(batch.delivered, 1, "relay 应交付一条事件：{batch:?}");
    assert_eq!(
        trace.entries(),
        vec!["allocate", "scan", "publish"],
        "补号必须发生在目录扫描与推送之前"
    );
}

#[tokio::test]
async fn relay_keeps_delivering_when_allocation_fails() {
    let trace = Arc::new(Trace::default());
    let relay = relay(
        Arc::clone(&trace),
        true,
        vec!["auction:1f0c0000-0000-4000-8000-000000000002".to_owned()],
    );
    let batch = relay.run_once(Utc::now()).await.expect("补号失败不得让 relay 轮次失败");
    assert_eq!(batch.delivered, 1);
    assert_eq!(batch.failures, 0);
    assert_eq!(trace.entries(), vec!["allocate", "scan", "publish"]);
}

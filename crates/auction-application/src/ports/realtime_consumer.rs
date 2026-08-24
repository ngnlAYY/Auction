//! Worker 实时事件消费端口。
//!
//! 消费端口只表达持久化游标、失败记录和显式重放边界。外部广播协议不属于这里；具体
//! sink 必须在本地事实提交后由适配器注入，不能让 Worker 直接依赖 HTTP 或数据库实现。

use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{ApplicationFuture, PublicRealtimeEvent};

/// 单次实时消费允许读取的最大事件数。
pub const MAX_REALTIME_CONSUMER_BATCH_SIZE: u16 = 200;

/// 实时消费的一次批量读取请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeConsumerRequest {
    pub consumer_key: String,
    pub stream_key: String,
    pub batch_size: u16,
    /// 是否允许消费 Public 进程使用的持久化 discovery 派生流。
    ///
    /// 外部广播 consumer 必须保持为 `false`，避免把拍卖创建事件从原始流和派生流重复
    /// 转发；Public 本地 relay 才能显式打开该开关。
    pub allow_discovery_stream: bool,
}

impl RealtimeConsumerRequest {
    /// 校验持久化游标的命名和批量边界。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.consumer_key.trim().is_empty()
            || self.consumer_key != self.consumer_key.trim()
            || self.consumer_key.len() > 128
            || self.consumer_key.chars().any(char::is_control)
            || self.stream_key.trim().is_empty()
            || self.stream_key != self.stream_key.trim()
            || self.stream_key.len() > 255
            || self.stream_key.chars().any(char::is_control)
            || self.batch_size == 0
            || self.batch_size > MAX_REALTIME_CONSUMER_BATCH_SIZE
        {
            return Err("实时消费游标参数无效");
        }
        // `auction:discovery` 是拍卖专属事件的持久化派生流。外部 sink 必须消费
        // `auction:<auction_id>` 原始流，否则会把同一创建事件重复转发到广播系统；Public
        // 本地 relay 则消费已经在创建事务中落库的派生流。
        if self.stream_key == "auction:discovery" && !self.allow_discovery_stream {
            return Err("公开发现派生流不能作为外部广播消费源");
        }
        Ok(())
    }
}

/// Worker 从持久化事件表读取的一批事件。
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeConsumerBatch {
    pub consumer_key: String,
    pub stream_key: String,
    pub after: i64,
    pub events: Vec<PublicRealtimeEvent>,
    /// `true` 表示游标之后的首个事件不是紧邻序号；消费方必须停下并进入恢复/重放流程。
    pub sequence_gap: bool,
}

/// 实时消费持久化端口。
pub trait RealtimeConsumerStore: Send + Sync {
    /// 读取当前游标之后的事件；没有对应的持久化 stream 时返回 `None`。
    fn read_batch(
        &self,
        request: RealtimeConsumerRequest,
    ) -> ApplicationFuture<'_, Option<RealtimeConsumerBatch>>;

    /// 只允许按当前游标的下一个序号推进，并将该事件的未解决失败标记为已解决。
    fn acknowledge(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        stream_sequence: i64,
    ) -> ApplicationFuture<'_, ()>;

    /// 写入或更新未解决的失败记录；失败不会推进游标。
    fn record_failure(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        failure_code: String,
        safe_detail: String,
        failed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 在确认替代事件已经追加到同一 stream 后，显式越过原失败事件。
    ///
    /// 原事件仍保留；游标只越过原事件的序号，随后仍按 stream 顺序消费替代事件及其间的
    /// 其他事件，避免通过重放偷偷重排历史序列。
    fn resolve_failure_after_replay(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        replay_event_id: Uuid,
        resolved_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;
}

/// Public relay 用于发现全部拍卖 stream 的持久化目录端口。
pub trait RealtimeStreamStore: Send + Sync {
    /// 按稳定 stream key 分页返回指定前缀后的 stream；结果必须按字典序递增。
    fn list_stream_keys(
        &self,
        prefix: String,
        after: String,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<String>>;
}

/// 外部广播/转发适配器报告的安全失败分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSinkError {
    pub failure_code: String,
    pub safe_detail: String,
}

/// 已提交实时事件的外部 sink 端口。
///
/// 实现必须以 `event_id` 做幂等键。Worker 可能在 sink 已收到事件但游标提交失败后再次
/// 投递同一事件；这种重复是 at-least-once 语义下的必要保护，而不是可忽略的异常。
pub trait RealtimeEventSink: Send + Sync {
    fn publish(
        &self,
        event: PublicRealtimeEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), RealtimeSinkError>> + Send + '_>>;
}

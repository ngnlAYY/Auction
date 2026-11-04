//! Public 进程内 realtime Hub。
//! Hub 只承担当前 Public 实例的有界连接分发；持久化 stream、消费游标和失败记录仍由
//! `auction-application`/`auction-storage` 负责。慢连接在发送队列满时立即淘汰，避免阻塞
//! relay 和其他连接。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use auction_application::{PublicRealtimeEvent, RealtimeEventSink, RealtimeSinkError};
use auction_observability::{RealtimeRecorder, RealtimeScope};
use axum::extract::ws::Utf8Bytes;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc::{self, Receiver, Sender};
use uuid::Uuid;

mod connections;
mod observation;

pub(crate) use observation::ObservedFrame;

use connections::{ConnectionLimits, ConnectionPermit};

/// 订阅拒绝分类；HTTP 将容量不足映射为 429，其余依赖问题映射为 503。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RealtimeSubscriptionError {
    /// 进程或用户的活跃连接额度已用尽。
    #[error("实时连接数已达上限")]
    Capacity,
    /// stream、身份或 Hub 状态无法安全使用。
    #[error("实时订阅不可用")]
    Unavailable,
}

/// Hub 运行水位；不包含 stream 名、事件内容或用户身份。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RealtimeHubHealth {
    /// 从 Hub 创建过的订阅总数。
    pub subscriptions: u64,
    /// 当前仍有连接的 stream 数量。
    pub open_streams: usize,
}

struct HubState {
    next_id: u64,
    streams: BTreeMap<String, BTreeMap<u64, Sender<ObservedFrame>>>,
}

/// 单个 Public 进程拥有的 stream 广播器。
pub struct RealtimeHub {
    client_buffer: usize,
    connections: Arc<ConnectionLimits>,
    state: Mutex<HubState>,
    metrics: RealtimeRecorder,
}

impl RealtimeHub {
    /// 创建不启动任务的有界 Hub。
    pub fn new(client_buffer: usize) -> Result<Self, &'static str> {
        if client_buffer == 0 || client_buffer > 1024 {
            return Err("实时客户端缓冲区无效");
        }
        Ok(Self {
            client_buffer,
            connections: Arc::new(ConnectionLimits::default()),
            state: Mutex::new(HubState { next_id: 0, streams: BTreeMap::new() }),
            metrics: RealtimeRecorder::default(),
        })
    }

    /// 注入 Public 进程观测端口；不改变配额或帧路由。
    pub fn with_metrics(mut self, metrics: RealtimeRecorder) -> Self {
        self.metrics = metrics;
        self
    }

    /// 为已认证用户的明确 stream 建立独占订阅；分配缓冲区前先取得双重连接配额。
    pub fn subscribe(
        self: &Arc<Self>,
        stream_key: String,
        user_id: Uuid,
    ) -> Result<RealtimeSubscription, RealtimeSubscriptionError> {
        validate_stream_key(&stream_key).map_err(|diagnostic_cause| {
            diagnostics::error(
                "realtime_hub",
                "validate_stream_key",
                diagnostics::invalid_state(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            RealtimeSubscriptionError::Unavailable
        })?;
        let connection_permit = self.connections.acquire(user_id)?;
        let (sender, receiver) = mpsc::channel(self.client_buffer);
        let mut state = self.state.lock().map_err(|diagnostic_cause| {
            diagnostics::error(
                "realtime_hub",
                "lock_subscriptions",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            RealtimeSubscriptionError::Unavailable
        })?;
        state.next_id =
            state.next_id.checked_add(1).ok_or(RealtimeSubscriptionError::Unavailable)?;
        let id = state.next_id;
        state.streams.entry(stream_key.clone()).or_default().insert(id, sender);
        Ok(RealtimeSubscription {
            hub: Arc::clone(self),
            stream_key,
            id,
            receiver,
            _connection_permit: connection_permit,
        })
    }

    /// 非阻塞广播已编码帧；队列已满或连接已关闭的订阅会被淘汰。
    fn broadcast(&self, stream_key: &str, frame: ObservedFrame) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(clients) = state.streams.get_mut(stream_key) else {
            return;
        };
        // 帧正文按引用计数共享；满/闭队列原位淘汰，不另建 stale 列表或再次查找订阅。
        clients.retain(|_, sender| {
            let Ok(permit) = sender.try_reserve() else {
                return false;
            };
            permit.send(frame.clone());
            true
        });
        if clients.is_empty() {
            state.streams.remove(stream_key);
        }
    }

    /// 返回 Hub 的安全运行水位。
    pub fn health(&self) -> RealtimeHubHealth {
        let Ok(state) = self.state.lock() else {
            return RealtimeHubHealth::default();
        };
        RealtimeHubHealth { subscriptions: state.next_id, open_streams: state.streams.len() }
    }

    /// 关闭新订阅入口；已取得许可的升级任务仍由连接自身发送既有关闭帧。
    pub(crate) fn begin_shutdown(&self) {
        self.connections.begin_shutdown();
    }

    /// 等待所有升级任务释放许可；时间预算和强制退出继续由进程 owner 统一持有。
    pub(crate) async fn drain(&self) -> Result<(), RealtimeSubscriptionError> {
        self.connections.wait_drained().await
    }

    fn remove(&self, stream_key: &str, id: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(clients) = state.streams.get_mut(stream_key) else {
            return;
        };
        clients.remove(&id);
        if clients.is_empty() {
            state.streams.remove(stream_key);
        }
    }
}

/// 一个 WebSocket 连接对应的有界订阅。
pub struct RealtimeSubscription {
    hub: Arc<RealtimeHub>,
    stream_key: String,
    id: u64,
    receiver: Receiver<ObservedFrame>,
    // Hub 淘汰发送端时不能提前归还槽位；消费者真正结束前仍占用连接资源。
    _connection_permit: ConnectionPermit,
}

impl RealtimeSubscription {
    /// 等待下一个已提交、已校验并编码的 v1 事件帧；淘汰后不再排空旧缓冲。
    pub async fn recv(&mut self) -> Option<Utf8Bytes> {
        self.recv_observed().await.map(|frame| frame.body)
    }

    pub(crate) fn observe_push(&self, scope: RealtimeScope, created_at: DateTime<Utc>) {
        self.hub.metrics.push(scope, created_at);
    }

    pub(crate) async fn recv_observed(&mut self) -> Option<ObservedFrame> {
        // 满队列淘汰只会丢弃 sender；mpsc 默认仍先交付全部积压帧，可能让慢连接长期
        // 占用配额。关闭前后都复核，令等待中的接收也在淘汰后直接进入 1013 关闭路径。
        if self.receiver.is_closed() {
            return None;
        }
        let frame = self.receiver.recv().await;
        if self.receiver.is_closed() { None } else { frame }
    }
}

impl Drop for RealtimeSubscription {
    fn drop(&mut self) {
        self.hub.remove(&self.stream_key, self.id);
    }
}

/// 将已提交 Public 事件交给当前进程 Hub 的本地 sink。
#[derive(Clone)]
pub struct RealtimeHubSink {
    hub: Arc<RealtimeHub>,
}

impl RealtimeHubSink {
    /// 创建 Hub sink；它不是外部广播系统适配器。
    pub fn new(hub: Arc<RealtimeHub>) -> Self {
        Self { hub }
    }
}

impl RealtimeEventSink for RealtimeHubSink {
    fn publish(
        &self,
        event: PublicRealtimeEvent,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), RealtimeSinkError>> + Send + '_>,
    > {
        Box::pin(async move {
            // 注册表校验已覆盖流名、发现流事件种类及负载归属；Hub 只按 stream 精确路由，
            // 因此每个事件在这里校验并编码一次，连接任务不再逐条重复校验或序列化。
            event.validate().map_err(|detail| RealtimeSinkError {
                failure_code: "schema_invalid".to_owned(),
                safe_detail: detail.to_owned(),
            })?;
            let stream_key = event.stream_key.clone();
            let created_at = event.created_at;
            let scope = if stream_key == "auction:discovery" {
                RealtimeScope::Discovery
            } else {
                RealtimeScope::Auction
            };
            let frame =
                super::realtime::encode_websocket_frame(event).map_err(|diagnostic_cause| {
                    diagnostics::error(
                        "realtime_hub",
                        "encode_frame",
                        DiagnosticError::diagnostic_kind(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    RealtimeSinkError {
                        failure_code: "encode_failed".to_owned(),
                        safe_detail: "实时事件编码失败".to_owned(),
                    }
                })?;
            self.hub.broadcast(&stream_key, ObservedFrame { body: frame, created_at, scope });
            self.hub.metrics.relay(scope, created_at);
            Ok(())
        })
    }
}

fn validate_stream_key(stream_key: &str) -> Result<(), &'static str> {
    if !auction_domain::valid_text(stream_key, 255) || !stream_key.starts_with("auction:") {
        return Err("实时订阅 stream 无效");
    }
    Ok(())
}

//! Public 进程内 realtime Hub。
//! Hub 只承担当前 Public 实例的有界连接分发；持久化 stream、消费游标和失败记录仍由
//! `auction-application`/`auction-storage` 负责。慢连接在发送队列满时立即淘汰，避免阻塞
//! relay 和其他连接。

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use auction_application::{PublicRealtimeEvent, RealtimeEventSink, RealtimeSinkError};
use tokio::sync::mpsc::{self, Receiver, Sender, error::TrySendError};
use uuid::Uuid;

mod connections;

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
    streams: BTreeMap<String, BTreeMap<u64, Sender<PublicRealtimeEvent>>>,
}

/// 单个 Public 进程拥有的 stream 广播器。
pub struct RealtimeHub {
    client_buffer: usize,
    connections: Arc<ConnectionLimits>,
    state: Mutex<HubState>,
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
        })
    }

    /// 为已认证用户的明确 stream 建立独占订阅；分配缓冲区前先取得双重连接配额。
    pub fn subscribe(
        self: &Arc<Self>,
        stream_key: String,
        user_id: Uuid,
    ) -> Result<RealtimeSubscription, RealtimeSubscriptionError> {
        validate_stream_key(&stream_key).map_err(|_| RealtimeSubscriptionError::Unavailable)?;
        let connection_permit = self.connections.acquire(user_id)?;
        let (sender, receiver) = mpsc::channel(self.client_buffer);
        let mut state = self.state.lock().map_err(|_| RealtimeSubscriptionError::Unavailable)?;
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

    /// 非阻塞广播到同一 stream；队列已满或连接已关闭的订阅会被淘汰。
    pub fn broadcast(&self, event: PublicRealtimeEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(clients) = state.streams.get_mut(&event.stream_key) else {
            return;
        };
        let mut stale = Vec::new();
        for (&id, sender) in clients.iter() {
            match sender.try_send(event.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_) | TrySendError::Closed(_)) => stale.push(id),
            }
        }
        for id in stale {
            clients.remove(&id);
        }
        if clients.is_empty() {
            state.streams.remove(&event.stream_key);
        }
    }

    /// 返回 Hub 的安全运行水位。
    pub fn health(&self) -> RealtimeHubHealth {
        let Ok(state) = self.state.lock() else {
            return RealtimeHubHealth::default();
        };
        RealtimeHubHealth { subscriptions: state.next_id, open_streams: state.streams.len() }
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
    receiver: Receiver<PublicRealtimeEvent>,
    // Hub 淘汰发送端时不能提前归还槽位；消费者真正结束前仍占用连接资源。
    _connection_permit: ConnectionPermit,
}

impl RealtimeSubscription {
    /// 等待下一个已提交并广播的事件；通道关闭表示连接应当结束。
    pub async fn recv(&mut self) -> Option<PublicRealtimeEvent> {
        self.receiver.recv().await
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
            event.validate().map_err(|detail| RealtimeSinkError {
                failure_code: "schema_invalid".to_owned(),
                safe_detail: detail.to_owned(),
            })?;
            self.hub.broadcast(event);
            Ok(())
        })
    }
}

fn validate_stream_key(stream_key: &str) -> Result<(), &'static str> {
    if stream_key.trim().is_empty()
        || stream_key != stream_key.trim()
        || stream_key.len() > 255
        || stream_key.chars().any(char::is_control)
        || !stream_key.starts_with("auction:")
    {
        return Err("实时订阅 stream 无效");
    }
    Ok(())
}

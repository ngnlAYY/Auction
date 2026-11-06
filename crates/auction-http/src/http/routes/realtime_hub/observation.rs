//! 只在进程内携带观测元数据，浏览器仍收到原编码字节。

use auction_observability::RealtimeScope;
use axum::extract::ws::Utf8Bytes;
use chrono::{DateTime, Utc};

#[derive(Clone)]
pub(crate) struct ObservedFrame {
    pub(crate) body: Utf8Bytes,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) scope: RealtimeScope,
}

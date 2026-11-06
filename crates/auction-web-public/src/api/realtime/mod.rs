//! Public 持久化实时事件 API 客户端。
//! 浏览器只解码经过服务端事件注册表校验的安全信封。WebSocket 沿用旧版逐事件 wire；
//! 浏览器先建立连接再执行 HTTP catch-up，并在序列跳号时重新补齐，不在本地猜测权威状态。
//!
//! 子模块边界：`protocol` 只拥有 wire 页/错误与序列、URL 校验；`cache` 维护有界防重和
//! 版本栅栏；`subscription` 负责 HTTP catch-up 与 WebSocket 订阅生命周期。

mod cache;
mod protocol;
mod subscription;

#[cfg(test)]
mod tests;

pub(crate) use cache::RealtimeCache;
pub(crate) use protocol::{RealtimeEvent, RealtimePage};
pub(crate) use subscription::{subscribe_auction_realtime, subscribe_realtime};

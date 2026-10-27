//! Public 持久化实时事件 HTTP 适配器装配。
//! 列表读取、WebSocket 推送、会话复核、目标解析和事件安全映射分别由子模块拥有；路由层只使用本模块导出的
//! 两个稳定 handler，进程内 Hub 只复用同一 v1 帧编码。

mod list;
mod mapping;
mod session;
mod target;
mod websocket;

pub(super) use list::list;
pub(super) use mapping::encode_websocket_frame;
pub(super) use websocket::websocket;

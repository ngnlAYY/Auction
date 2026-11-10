//! Admin 论坛本地同步状态 HTTP 适配器。
//! 处理器只负责 Admin session、稳定 DTO 映射和错误状态；论坛主题解析由 Worker 的论坛客户端完成。

mod status;

pub(crate) use status::{get, sync_status};

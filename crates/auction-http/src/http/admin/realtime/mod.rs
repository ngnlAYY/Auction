//! Admin realtime HTTP 适配器的模块装配。
//!
//! 事件元数据、lag/failure 读取、replay 命令、安全投影映射和共享分页/错误边界分别由
//! 子模块拥有；本模块只保留路由可见的稳定处理器出口。

#[path = "common.rs"]
mod common;
#[path = "mapping.rs"]
mod mapping;
#[path = "reads.rs"]
mod reads;
#[path = "replay.rs"]
mod replay;

pub(super) use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_mutation,
    authenticated_admin_session, internal_error, no_store_headers, not_found_error,
    parse_positive_i64, parse_query, request_id, timestamp,
};
pub(super) use common::{
    feature_unavailable, invalid_query, invalid_replay, map_realtime_error, map_replay_error,
    parse_page, parse_uuid, replay_unavailable,
};
pub(super) use mapping::{map_event, map_failure, map_lag};
pub(super) use reads::{get_failure, list_events, list_failures, list_lag};
pub(super) use replay::replay;

const REPLAY_COMMAND_KIND: &str = "replay_realtime_failure";

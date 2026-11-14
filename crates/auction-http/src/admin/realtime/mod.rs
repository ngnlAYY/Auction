//! Admin realtime HTTP 适配器的模块装配。
//! 事件元数据、lag/failure 读取、replay 命令、安全投影映射和共享分页/错误边界分别由
//! 子模块拥有；本模块只转出路由可见的稳定处理器，以及子模块之间共用的分页/错误边界与
//! 安全投影映射，并定义 replay 命令账本的类型名。会话守卫与状态由子模块从定义处导入。

mod boundary;
mod mapping;
mod reads;
mod replay;

pub(crate) use boundary::{
    feature_unavailable, invalid_query, invalid_replay, map_realtime_error, map_replay_error,
    parse_page, replay_unavailable,
};
pub(crate) use mapping::{map_event, map_failure, map_lag};
pub(crate) use reads::{list_events, list_failures, list_lag};
pub(crate) use replay::replay;

const REPLAY_COMMAND_KIND: &str = "replay_realtime_failure";

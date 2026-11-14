//! Admin 邮件 HTTP 适配器的模块装配。
//! 轮询状态、入站安全投影、状态命令和 Worker 唤醒分别由子模块拥有，共享错误映射在
//! `errors`；本模块只转出路由可见的稳定处理器与子模块之间共用的错误映射。会话守卫与状态
//! 由子模块从定义处导入。

mod commands;
mod diagnostics;
mod errors;
mod intake;
mod state;
mod trigger;

pub(crate) use commands::set_paused;
pub(crate) use errors::{feature_unavailable, map_error};
pub(crate) use intake::{detail_intake, list_intakes};
pub(crate) use state::state;
pub(crate) use trigger::trigger_poll;

//! 论坛主题读取、固定楼层校验、身份解析和目录/系统日志刷新。
//!
//! - `scan`：单轮分页读取、回帖命令消费与系统日志投影。
//! - `refresh`：目录、续页与系统日志的渲染、发布和提交。
//! - `layout`：固定楼层身份校验与系统日志条目构造。
//! - `schedule`：下一次扫描时刻与 Final 阶段读取节奏。

mod layout;
mod refresh;
mod scan;
mod schedule;

pub(super) use scan::process_topic;

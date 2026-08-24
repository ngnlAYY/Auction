//! PostgreSQL Admin lot 更新 owner 的稳定装配。
//!
//! 事务适配、锁内读取、状态变更、withdrawn-return 义务和审计/realtime 事件分别由相邻
//! 模块拥有；所有调用仍运行在父事务内。

mod events;
mod mutation;
mod obligation;
mod read;
mod transaction;

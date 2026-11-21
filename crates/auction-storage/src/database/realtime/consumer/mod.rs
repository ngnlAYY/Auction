//! Public relay 实时事件消费的持久化适配器；stream 目录、事件读取、游标确认与 failure 生命周期
//! 由具名子模块持有。游标只按 stream 连续序号推进，失败记录不改它。
//!
//! 消费端口只由持锁 `RealtimeConsumerOwnership` 实现，连接池不提供任何消费入口：relay 断线后
//! 不能另取连接绕过互斥 session 继续确认游标。

mod acknowledgement;
pub(in crate::database) mod failure;
mod owned;
mod ownership;
mod read;
mod stream;
mod validation;

pub use ownership::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};

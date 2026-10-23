//! 现金sending门装配：授权、精确持久fence与类型化事务边界各自独立。

mod authorization;
mod persistence;
mod transaction;

pub(in crate::database::delivery_scheduler) use transaction::FenceError;
pub(super) use transaction::{run, run_legacy};

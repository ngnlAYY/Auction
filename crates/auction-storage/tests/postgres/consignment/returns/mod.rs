//! 退件接管（第 6 期 6D5）的 PG18 测试：按 K 接管、Take 起写的唯一许可与 fence 顺序、消费与资金回补。
//!
//! 只测核心规则：资产外写（起写许可只发一次）、金额与资金（回补同事务）、命令幂等、并发与原子性、
//! 安全边界；夹具复用 6B3 的现金退款存量。

mod acquire;
mod consume;
mod fence;
pub mod fixture;
mod intake_effects;
mod poll_origin;
mod release;
mod season_take;
mod stock_probe;
mod take_work;

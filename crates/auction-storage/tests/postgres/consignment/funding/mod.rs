//! 寄售资金支出侧（第 6 期 6B3）的 PG18 测试：现金退件阻断与回补、现金腿状态迁移、手续费与退款转换；
//! 收款侧（6B2）：COD 实收跨 opening 的唯一消费、opening 份额登记、record_only 收款。
//!
//! 只测核心规则：资金与金额、命令幂等、并发与原子性、安全边界；夹具复用 000136 的购买与退件接管夹具。

mod blockers;
mod carrier;
mod cash_legs;
mod certify;
mod certify_guards;
pub(in crate::consignment) mod cod_fixture;
mod fee;
pub mod fixture;
mod opening_slice;
mod order_event;
mod port;
mod preflight_uid;
mod preview;
mod record_only;
mod solvency;

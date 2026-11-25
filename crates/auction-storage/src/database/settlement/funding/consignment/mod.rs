//! 寄售资金存储：支出侧（第 6 期 6B3）——现金退件阻断与回补、现金腿状态迁移、费用与本金事实、
//! 退款转换与购前预检；收款侧（6B2）——COD 实收认证、opening 份额登记、收据与分账、record_only 收款。
//!
//! 资金表与资金 SQL 只有资金 owner 写；购买、退件接管、收款认证通过本模块的事务内原语调用，
//! 不直接写资金表。支出侧原语不提交、不外呼，外层事务与锁序由调用方遵守（D9 §1.3：
//! `runtime generation → funding worlds → UID 账户 → 来源业务行 → 凭据 → 受益用户 → obligations →
//! allocations → deliveries → fulfillments`）。收款侧入口都是顶层 Serializable 事务，同一锁序；
//! 序列化冲突与死锁只对已确认回滚的 40001 / 40P01 以原命令有界重试，提交结果未知交原命令 ID 核对。

pub(crate) mod blockers;
pub(crate) mod cancel_unpaid;
pub(crate) mod cash_legs;
pub(crate) mod certify_cod;
mod command_ledger;
mod commands;
mod contract_allocations;
mod distribution;
pub(crate) mod fee;
mod freeze;
mod fresh_cash;
pub(crate) mod opening_slice;
mod order_stage;
mod port;
pub(crate) mod preflight;
pub(crate) mod preview;
pub(crate) mod principal;
mod receipt;
mod record_only;
pub(crate) mod refund_conversion;
pub(crate) mod returns;
mod serializable;
#[cfg(feature = "test-db")]
mod test_hooks;
#[cfg(feature = "test-db")]
mod test_hooks_cod;
mod transaction;
mod world_leg;

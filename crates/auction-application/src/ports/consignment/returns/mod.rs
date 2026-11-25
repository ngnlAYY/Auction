//! 寄售退件接管的窄端口：按 RTS 的 K 接管、Take 起写与消费。
//!
//! 起写复用第 4 期的 `WriteStartOutcome` 与 `PostPermit`（同一起写原语）；消费同事务写证明、
//! 消费与资金回补。接管、起写与消费的判定规则在 `auction_domain::consignment::returns`，
//! 存储实现（第 6 期 6D5）在同一事务内落实。

mod commands;
mod store;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use commands::{
    AcquireReceipt, AcquireReturn, ConsumeReceipt, ConsumeReturn, ReturnOperation, StartReturnTake,
};
pub use store::ReturnIntakeStore;

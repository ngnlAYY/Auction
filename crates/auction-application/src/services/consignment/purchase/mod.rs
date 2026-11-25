//! 寄售购买与 v2 改价用例：规范摘要与命令组装。

mod commands;
pub mod encoding;

pub use commands::{PurchaseCommandService, purchase_funding};

#[cfg(test)]
mod tests;

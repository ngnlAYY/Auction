//! 寄售登记、分权查询、解析、加密和维护的窄端口。

mod commands;
mod contract;
pub(crate) mod deposit;
mod dispatch;
mod equipment;
mod errors;
mod maintenance;
mod orders;
mod owned_commands;
mod projection;
mod purchase;
mod reads;
mod reads_v2;
pub(crate) mod recovery;
mod returns;
pub(crate) mod rights;
mod work;

pub use commands::*;
pub use contract::*;
pub use deposit::*;
pub use dispatch::*;
pub use equipment::*;
pub use errors::*;
pub use maintenance::*;
pub use orders::*;
pub use owned_commands::*;
pub use projection::*;
pub use purchase::*;
pub use reads::*;
pub use reads_v2::*;
pub use recovery::*;
pub use returns::*;
pub use rights::*;
pub use work::*;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

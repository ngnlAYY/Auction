//! 寄售第 4 期收件与代管的 PostgreSQL 适配器。
//!
//! 预约、整封接受、取得后挂牌、代管控制与季任务都在 4K 的同一顶层事务里写业务 SQL；
//! 4C 的隔离/外写端口复用 `accept_items_in_tx` 与 `apply_take_custody_in_tx`。

mod controls;
pub(crate) mod crypto;
mod items;
mod outcome;
pub(crate) mod poll;
pub(crate) mod primitives;
mod replay;
pub(crate) mod repository;
mod reservations;
mod season_task;
mod seasons;
#[cfg(feature = "test-db")]
pub mod test_access;

#[allow(unused_imports, reason = "4C 在同一 crate 内接入取得后挂牌原语")]
pub(crate) use crate::database::consignment::custody::apply_take_custody_in_tx;
pub use auction_application::consignment::{
    ReservationSecretAad, ReservationSecretCrypto, ReservationSecretCryptoError,
    SealedReservationSecret,
};
#[allow(unused_imports, reason = "4C 在同一 crate 内使用逐件返回类型")]
pub(crate) use items::AcceptedDepositItem;
#[allow(unused_imports, reason = "4C 在同一 crate 内接入这些事务内原语")]
pub(crate) use items::accept_items_in_tx;
pub use repository::{DepositRepository, DepositSettings};

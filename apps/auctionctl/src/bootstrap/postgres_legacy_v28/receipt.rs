//! PostgreSQL v28 接管 receipt 的私有预留、原子发布和失败清理。

use super::super::{LegacyV28AdoptionReceiptSpec, PrivateReceiptReservation};

/// PostgreSQL v28 接管使用的不可覆盖回执预留器。
pub(super) type AdoptionReceiptReservation =
    PrivateReceiptReservation<LegacyV28AdoptionReceiptSpec>;

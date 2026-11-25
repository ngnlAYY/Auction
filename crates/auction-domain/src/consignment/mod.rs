//! 一口价挂单的领域模型、资格、状态迁移与定价的公共入口。

pub mod contract;
pub mod deposit;
pub mod dispatch;
mod eligibility;
mod errors;
mod identity;
mod listing;
mod order_stage;
mod pricing;
pub mod purchase;
pub mod recovery;
pub mod refusal;
pub mod returns;
pub mod rights;
mod transition;

pub use contract::*;
pub use deposit::*;
pub use eligibility::*;
pub use errors::*;
pub use identity::*;
pub use listing::*;
pub use order_stage::*;
pub use pricing::*;
pub use transition::*;

pub use crate::delivery::{OriginalMailKey, parse_settlement_world, settlement_world_code};
pub use crate::scheduling::{
    CalendarVersion, ConsignmentCalendar, FrozenSeason, MAX_PRECLOSE_DAYS, SeasonError,
    SeasonState, season_state,
};
pub use crate::settlement::consignment_funding as funding;

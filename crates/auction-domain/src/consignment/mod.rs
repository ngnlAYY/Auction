//! 一口价挂单的领域模型、资格、状态迁移与定价的公共入口。

pub mod deposit;
mod eligibility;
mod errors;
mod identity;
mod listing;
mod pricing;
mod transition;

pub use deposit::*;
pub use eligibility::*;
pub use errors::*;
pub use identity::*;
pub use listing::*;
pub use pricing::*;
pub use transition::*;

pub use crate::delivery::{OriginalMailKey, parse_settlement_world, settlement_world_code};
pub use crate::scheduling::{
    CalendarVersion, ConsignmentCalendar, FrozenSeason, MAX_PRECLOSE_DAYS, SeasonError,
    SeasonState, season_state,
};

//! 竞价规划器的模块装配。

mod plan;
mod trigger;

pub use plan::CompetitionPlan;
pub use trigger::{CompetitionTrigger, CompetitionTriggerKind};

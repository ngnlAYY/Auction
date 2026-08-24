//! Recovery action 的生命周期模块装配。
//!
//! 领取、投递状态推进和终态收口分别位于子模块；所有实现仍由 recovery scheduler
//! 统一通过 `RecoveryWorkStore` 暴露，避免把外部副作用带入存储层。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[path = "actions/claim.rs"]
mod claim;
#[cfg(feature = "postgres")]
#[path = "actions/delivery/mod.rs"]
mod delivery;
#[cfg(feature = "postgres")]
#[path = "actions/resolution.rs"]
mod resolution;

#[cfg(feature = "postgres")]
pub(super) use claim::claim_action_postgres;
#[cfg(feature = "postgres")]
pub(super) use delivery::{
    link_action_postgres, mark_action_enqueued_postgres, observe_action_postgres,
};
#[cfg(feature = "postgres")]
pub(super) use resolution::{
    manual_action_postgres, mark_action_satisfied_postgres, release_action_postgres,
};

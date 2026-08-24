//! Recovery action 投递状态推进与结果观察的模块装配。
//!
//! pending → prepared → enqueued 的身份推进与终态证明分别由子模块负责；
//! 本模块只保留稳定出口，避免把不同阶段的 fence 逻辑重新混在一个文件中。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[path = "activation.rs"]
mod activation;
#[cfg(feature = "postgres")]
#[path = "observation.rs"]
mod observation;
#[cfg(feature = "postgres")]
#[path = "prepare.rs"]
mod prepare;

#[cfg(feature = "postgres")]
pub(crate) use activation::mark_action_enqueued_postgres;
#[cfg(feature = "postgres")]
pub(crate) use observation::observe_action_postgres;
#[cfg(feature = "postgres")]
pub(crate) use prepare::link_action_postgres;

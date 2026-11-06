//! Recovery 锁定范围、状态校验与履约证据的模块装配；锁定、回放和证据验证仍在同一事务内执行。

#[cfg(feature = "postgres")]
#[path = "scope/locks.rs"]
mod locks;
#[cfg(feature = "postgres")]
#[path = "scope/model.rs"]
mod model;
#[cfg(feature = "postgres")]
#[path = "scope/replay.rs"]
mod replay;
#[cfg(feature = "postgres")]
#[path = "scope/validation.rs"]
mod validation;

#[cfg(feature = "postgres")]
pub(super) use locks::*;
#[cfg(feature = "postgres")]
pub(super) use model::*;
#[cfg(feature = "postgres")]
pub(super) use replay::*;
#[cfg(feature = "postgres")]
pub(super) use validation::*;

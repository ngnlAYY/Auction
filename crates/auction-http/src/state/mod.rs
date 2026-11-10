//! Public/Admin HTTP state 的表面级装配。
//! 两个 state 容器分别由 Public 与 Admin 子模块拥有，本模块统一导出。

mod admin;
mod public;

pub use admin::AdminApiState;
pub use public::PublicApiState;

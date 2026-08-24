//! Public/Admin HTTP state 的表面级装配。
//!
//! 两个 state 容器分别由 Public 与 Admin 子模块拥有；这里保留原有
//! `http::shared::state::{PublicApiState, AdminApiState}` 路径。

mod admin;
mod public;

pub use admin::AdminApiState;
pub use public::PublicApiState;

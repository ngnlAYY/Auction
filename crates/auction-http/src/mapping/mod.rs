//! HTTP 响应映射按查询、Public、Admin 和目录投影分组。

mod admin;
mod catalog;
mod public;
mod query;

pub(crate) use admin::*;
pub(crate) use catalog::*;
pub(crate) use public::*;
pub(crate) use query::*;

//! MoogleMail HTML 文档解析器的职责分组。

/// 无状态的 MoogleMail HTML 解析器。
#[derive(Debug, Clone, Copy, Default)]
pub struct MoogleMailParser;

mod actions;
mod common;
mod detail;
mod list_helpers;
mod reads;
mod script;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use common::*;
pub(crate) use detail::*;
pub(crate) use list_helpers::*;
pub(crate) use reads::MoogleMailInventoryPreparation;
pub(crate) use script::*;
pub use types::*;

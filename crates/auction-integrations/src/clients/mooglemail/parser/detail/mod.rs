//! MoogleMail 详情页、附件和动作证明解析的职责分组。

mod fields;
mod parse;
mod receipt;

pub(crate) use fields::*;
pub(crate) use parse::*;
pub(crate) use receipt::*;

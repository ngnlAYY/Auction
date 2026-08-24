//! MoogleMail 写入证明解析器的稳定出口。
//!
//! compose 状态、compose 资产校验和最终响应分类分别由子模块拥有；Cookie、页面 token 和
//! 原始 HTML 仍只在 integrations 边界内短暂存在。

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailMessageFields, MoogleMailOperation,
    MoogleMailParser, MoogleMailToken, MoogleMailWorld,
    protocol::{valid_decimal_id, valid_line},
};

#[path = "writer/common.rs"]
mod common;
#[path = "writer/compose.rs"]
mod compose;
#[path = "writer/model.rs"]
mod model;
#[path = "writer/response.rs"]
mod response;

pub use model::{MoogleMailComposeState, MoogleMailSendResponseFailure};

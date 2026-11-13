//! MoogleMail 写入证明解析器的稳定出口；Cookie、页面 token 和原始 HTML 只在
//! integrations 边界内短暂存在。

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailMessageFields, MoogleMailOperation,
    MoogleMailParser, MoogleMailToken, MoogleMailWorld,
    protocol::{valid_decimal_id, valid_line},
};

mod common;
mod compose;
mod model;
mod response;

pub use model::{MoogleMailComposeState, MoogleMailSendResponseFailure};

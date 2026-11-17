//! 稳定 HTTP/流 DTO 与错误契约的薄 crate 入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod dto;
mod setup;

pub use dto::*;
pub use setup::*;

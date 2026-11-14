//! MoogleMail 业务处理端口。
//! 该模块承接已通过 MoogleMail 协议解析的短生命周期业务 envelope。正文和装备 key 只在
//! Worker 调用应用用例期间存在；进入持久化端口前必须转换为装备事实、密钥信封和无密文
//! delivery payload。

mod acceptance;
mod auction;
mod decision;
mod delivery;
mod errors;
mod filter;
mod message;

pub use acceptance::*;
pub use auction::*;
pub use decision::*;
pub use delivery::*;
pub use errors::{MailProcessingError, mail_auction_aggregate_id};
pub use filter::*;
pub use message::*;

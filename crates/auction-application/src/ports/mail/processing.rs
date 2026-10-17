//! MoogleMail 业务处理端口。
//! 该模块承接已通过 MoogleMail 协议解析的短生命周期业务 envelope。正文和装备 key 只在
//! Worker 调用应用用例期间存在；进入持久化端口前必须转换为装备事实、密钥信封和无密文
//! delivery payload。

#[path = "processing/acceptance.rs"]
mod acceptance;
#[path = "processing/auction.rs"]
mod auction;
#[path = "processing/common.rs"]
mod common;
#[path = "processing/decision.rs"]
mod decision;
#[path = "processing/delivery.rs"]
mod delivery;
#[path = "processing/filter.rs"]
mod filter;
#[path = "processing/message.rs"]
mod message;

pub use acceptance::*;
pub use auction::*;
pub use common::{MailProcessingError, mail_auction_aggregate_id};
pub use decision::*;
pub use delivery::*;
pub use filter::*;
pub use message::*;

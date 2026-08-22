//! HentaiVerse MoogleMail 的受限读取与写入适配器。
//!
//! 该模块只负责固定远端协议、HTTP 传输、HTML 解析、写入证明和凭据边界。解析成功后返回
//! 不含原始 HTML 的 typed envelope；页面 token 只能在短生命周期表单构造期间使用，Cookie
//! 不进入返回值。应用层仍需自行完成邮件分类、幂等入库和业务命令编排。

mod client;
mod error;
mod forms;
mod gateway;
mod parser;
mod protocol;
mod sender;
mod writer;

pub use client::{MoogleMailClient, MoogleMailClientConfig};
pub use error::{MoogleMailError, MoogleMailErrorCategory};
pub use forms::{MoogleMailForm, MoogleMailMessageFields};
pub use gateway::{
    MoogleMailCredentialSet, MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailPage,
};
pub use parser::{
    MoogleMailAttachment, MoogleMailInboxPage, MoogleMailMessage, MoogleMailParser,
    MoogleMailSummary,
};
pub use protocol::{
    MoogleMailCookie, MoogleMailEquipmentKey, MoogleMailOperation, MoogleMailTarget,
    MoogleMailToken, MoogleMailWorld,
};
pub use sender::{MoogleMailSendReceipt, MoogleMailSendRequest, MoogleMailSender};
pub use writer::MoogleMailComposeState;

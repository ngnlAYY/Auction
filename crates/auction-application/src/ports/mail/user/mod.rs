//! Users owner 的 MoogleMail 注册与换绑端口。
//!
//! 该模块只定义跨层契约：登录 key 明文只在 Worker 的短生命周期值对象中存在，数据库只接收
//! 摘要和密文信封，Delivery outbox 只接收用户 ID 引用。具体的密钥生成、加解密和 SQL 由
//! integrations/storage owner 分别实现。

mod commands;
mod model;
mod ports;
mod security;

pub use commands::{MailUserRebindCommand, MailUserRegisterCommand, MailUserRegisterWrite};
pub use model::{MailUserCommandKind, MailUserResult, MailUserResultCode};
pub use ports::{
    MailLoginKeyEnvelopeReader, MailLoginKeyIssuer, MailLoginKeyReader, MailUserPort, MailUserStore,
};
pub use security::{MailLoginKey, MailLoginKeyEnvelope, MailLoginKeyIssue};

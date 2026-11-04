//! Users/MoogleMail 注册与换绑命令载荷。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::RequestHash;
use super::security::MailLoginKeyEnvelope;

#[derive(Debug, Clone)]
pub struct MailUserRegisterCommand {
    pub command_id: auction_domain::CommandId,
    pub request_hash: RequestHash,
    pub username: String,
    pub now: DateTime<Utc>,
}

/// Users owner 原子注册写入；信封 AAD 绑定 user_id，锁内实际用户 UUID 必须与之相同。
pub struct MailUserRegisterWrite {
    pub command: MailUserRegisterCommand,
    pub user_id: Uuid,
    pub public_key_hash: [u8; 32],
    pub envelope: MailLoginKeyEnvelope,
}

/// 换绑命令只接收旧 key 摘要，明文在进入 Users owner 前已经清零。
#[derive(Debug, Clone)]
pub struct MailUserRebindCommand {
    pub command_id: auction_domain::CommandId,
    pub request_hash: RequestHash,
    pub old_key_hash: [u8; 32],
    pub next_username: String,
    pub history_id: Uuid,
    pub now: DateTime<Utc>,
}

/// 自助轮换登录 key 的命令载荷；账号身份来自发件人游戏名。
#[derive(Debug, Clone)]
pub struct MailUserRekeyCommand {
    pub command_id: auction_domain::CommandId,
    pub request_hash: RequestHash,
    pub username: String,
    pub now: DateTime<Utc>,
}

/// Users owner 原子轮换写入；轮换新 key、注销全部有效会话与命令账本必须同事务提交。
/// 信封 AAD 绑定 user_id，锁内实际用户 UUID 必须与之相同。
pub struct MailUserRekeyWrite {
    pub command: MailUserRekeyCommand,
    pub user_id: Uuid,
    pub public_key_hash: [u8; 32],
    pub envelope: MailLoginKeyEnvelope,
}

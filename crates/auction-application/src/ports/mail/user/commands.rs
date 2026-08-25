//! Users/MoogleMail 注册与换绑命令载荷。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::RequestHash;
use super::security::MailLoginKeyEnvelope;

/// 注册或找回命令。
#[derive(Debug, Clone)]
pub struct MailUserRegisterCommand {
    pub command_id: auction_domain::CommandId,
    pub request_hash: RequestHash,
    pub username: String,
    pub game_player_id: String,
    pub now: DateTime<Utc>,
}

/// Users owner 原子注册写入；候选用户与密钥信封必须在同一事务中提交。
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
    pub next_game_player_id: String,
    pub history_id: Uuid,
    pub now: DateTime<Utc>,
}

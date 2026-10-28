pub(super) use auction_domain::valid_text;
use thiserror::Error;
use uuid::Uuid;

/// MoogleMail 业务处理错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum MailProcessingError {
    #[error("MoogleMail 业务 envelope 无效")]
    InvalidMessage,
    #[error("MoogleMail 业务处理配置无效")]
    InvalidConfiguration,
    #[error("MoogleMail 邮件无法安全退回或领取")]
    UnsafeDisposition,
    #[error("MoogleMail 装备来源不可信")]
    InvalidEquipment,
    #[error("MoogleMail owner 返回结果无效")]
    OwnerResultInvalid,
    #[error("MoogleMail 业务决定无效")]
    InvalidDecision,
    #[error("MoogleMail 拍卖入站命令无效")]
    InvalidAuctionCommand,
    #[error("MoogleMail Users 命令无效")]
    InvalidUserCommand,
    #[error("MoogleMail 拍卖入站密钥封装失败")]
    KeyUnavailable,
    #[error("MoogleMail 拍卖入站命令与已有请求冲突")]
    CommandConflict,
    #[error("MoogleMail 拍卖入站存储结果损坏")]
    StoredResultCorrupt,
    #[error("MoogleMail 登录 key 无效")]
    InvalidLoginKey,
    #[error("MoogleMail 登录 key 不存在")]
    LoginKeyNotFound,
    #[error("MoogleMail 登录 key 信封无效")]
    InvalidLoginKeyEnvelope,
    #[error("Users owner 返回结果无效")]
    InvalidUserResult,
}

pub(super) fn decimal_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn clear_string(value: &mut String) {
    let mut bytes = std::mem::take(value).into_bytes();
    bytes.fill(0);
}

/// 生成 world-scoped MoogleMail 拍卖命令账本聚合标识。
pub fn mail_auction_aggregate_id(world_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/mail-auction-intake/{world_id}").as_bytes())
}

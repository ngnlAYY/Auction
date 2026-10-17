//! 邮件拍卖已接受决定的恢复身份和无密钥资产投影。

use auction_domain::CommandId;
use chrono::{DateTime, Utc};

use super::{
    auction::MailAuctionIntakeResult,
    common::{MailProcessingError, decimal_identity, valid_text},
};

/// 在重新读取可变远端装备前查询已经提交的拍卖命令。
/// 正文摘要和当前附件列表会因领取而变化，不能作为已接受决定的恢复依据；持久化投稿的
/// 邮箱、世界、邮件 ID、收件时间和原发件人必须全部匹配，原附件集合由存储事实恢复。
#[derive(Clone)]
pub struct MailAuctionIntakeLookup {
    pub command_id: CommandId,
    pub world_id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub seller: String,
    pub received_at: DateTime<Utc>,
}

impl MailAuctionIntakeLookup {
    /// 拒绝无法绑定原邮件身份的恢复查询。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if self.command_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || !valid_text(&self.mailbox_key, 256)
            || !valid_text(&self.external_mail_id, 512)
            || !valid_text(&self.seller, 256)
            || self.received_at.timestamp_nanos_opt().is_none()
        {
            return Err(MailProcessingError::InvalidAuctionCommand);
        }
        Ok(())
    }
}

/// 从已提交命令和投稿资产恢复的决定；只用于补齐原领取意图，不能重新生成录入命令。
pub struct MailAuctionAcceptance {
    pub result: MailAuctionIntakeResult,
    pub attachment_ids: Vec<String>,
}

impl MailAuctionAcceptance {
    /// 原附件集合必须完整、有序且唯一，避免给既有 take 命令生成不同载荷。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        self.result.validate()?;
        if self.attachment_ids.len() != self.result.lot_ids.len()
            || self.attachment_ids.len() > 1000
            || self.attachment_ids.iter().any(|value| !decimal_identity(value))
            || self.attachment_ids.windows(2).any(|values| values[0] >= values[1])
        {
            return Err(MailProcessingError::StoredResultCorrupt);
        }
        Ok(())
    }
}

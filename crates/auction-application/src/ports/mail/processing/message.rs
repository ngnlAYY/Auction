use std::fmt;

use chrono::{DateTime, Utc};

use auction_domain::EquipmentKey;

use super::{
    super::super::{AdminIntakeSourceSnapshot, ApplicationFuture},
    common::{MailProcessingError, clear_string, decimal_identity, valid_text},
};

/// MoogleMail 业务 envelope 中单件装备附件。
///
/// key 只用于读取可信装备页和生成密钥信封，禁止实现任何会输出 key 的格式化或序列化。
pub struct MailEquipmentAttachment {
    pub external_equipment_id: String,
    pub key: EquipmentKey,
}

impl fmt::Debug for MailEquipmentAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailEquipmentAttachment")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl MailEquipmentAttachment {
    /// 清零短生命周期装备 key。
    pub fn clear_secret(&mut self) {
        self.key.clear();
    }

    /// 为同一封邮件的有界幂等重试建立短生命周期附件副本。
    fn duplicate_for_retry(&self) -> Self {
        Self {
            external_equipment_id: self.external_equipment_id.clone(),
            key: self.key.duplicate_for_retry(),
        }
    }
}

/// 已由外部协议适配器校验的 MoogleMail 业务 envelope。
///
/// 该类型不允许进入数据库或持久化命令。Drop 会尽力清除正文、发件人、标题和附件 key。
pub struct MailMessage {
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub received_at: DateTime<Utc>,
    pub sender: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: i64,
    pub attachments: Vec<MailEquipmentAttachment>,
    pub returnable: bool,
    pub claimable: bool,
    pub content_digest: [u8; 32],
}

impl fmt::Debug for MailMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailMessage")
            .field("mailbox_key", &self.mailbox_key)
            .field("external_mail_id", &self.external_mail_id)
            .field("world_id", &self.world_id)
            .field("received_at", &self.received_at)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachments.len())
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .field("content_digest", &self.content_digest)
            .finish()
    }
}

impl Drop for MailMessage {
    fn drop(&mut self) {
        self.clear_secrets();
    }
}

impl MailMessage {
    /// 校验 Worker 到应用边界的 envelope，拒绝不完整或越界的业务输入。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if !valid_text(&self.mailbox_key, 256)
            || !valid_text(&self.external_mail_id, 512)
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.received_at.timestamp_nanos_opt().is_none()
            || !valid_text(&self.sender, 256)
            || !valid_text(&self.subject, 512)
            || self.body.len() > 64 * 1024
            || self
                .body
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            || self.credits < 0
            || self.cod < 0
            || self.content_digest == [0; 32]
            || self.attachments.is_empty() && self.cod > 0 && self.credits == 0
            || self.attachments.len() > 1000
        {
            return Err(MailProcessingError::InvalidMessage);
        }
        for (index, attachment) in self.attachments.iter().enumerate() {
            if !decimal_identity(&attachment.external_equipment_id)
                || index > 0
                    && self.attachments[..index].iter().any(|previous| {
                        previous.external_equipment_id == attachment.external_equipment_id
                    })
            {
                return Err(MailProcessingError::InvalidMessage);
            }
        }
        Ok(())
    }

    /// 清除当前 envelope 持有的敏感短生命周期数据。
    pub fn clear_secrets(&mut self) {
        clear_string(&mut self.sender);
        clear_string(&mut self.subject);
        clear_string(&mut self.body);
        for attachment in &mut self.attachments {
            attachment.clear_secret();
        }
    }

    /// 为同一稳定命令 ID 的有界进程内重试建立显式副本。
    ///
    /// 该类型刻意不实现 Clone；只有 Worker 的有限次数重试可以调用此入口，并且任意时刻最多
    /// 保留原 envelope 与一个执行副本。所有副本仍通过 Drop 清除正文、身份和装备 key。
    pub fn duplicate_for_retry(&self) -> Self {
        Self {
            mailbox_key: self.mailbox_key.clone(),
            external_mail_id: self.external_mail_id.clone(),
            world_id: self.world_id.clone(),
            received_at: self.received_at,
            sender: self.sender.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
            credits: self.credits,
            cod: self.cod,
            attachments: self
                .attachments
                .iter()
                .map(MailEquipmentAttachment::duplicate_for_retry)
                .collect(),
            returnable: self.returnable,
            claimable: self.claimable,
            content_digest: self.content_digest,
        }
    }

    /// 返回排序后的外部装备 ID，用于稳定 delivery payload 和审计元数据。
    pub fn sorted_attachment_ids(&self) -> Vec<String> {
        let mut ids = self
            .attachments
            .iter()
            .map(|attachment| attachment.external_equipment_id.clone())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }
}

/// MoogleMail 附件解析得到的可信装备读取器。
pub trait MailEquipmentReader: Send + Sync {
    fn read(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;
}

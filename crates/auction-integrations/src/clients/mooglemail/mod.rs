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
mod transport;
mod writer;

pub use client::{MoogleMailClient, MoogleMailClientConfig};
pub use error::{MoogleMailError, MoogleMailErrorCategory};
pub use forms::{MoogleMailForm, MoogleMailMessageFields};
pub use gateway::{
    MoogleMailCredentialSet, MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailPage,
};
pub use parser::{
    MoogleMailActionState, MoogleMailAttachment, MoogleMailBalanceSnapshot, MoogleMailInboxPage,
    MoogleMailMessage, MoogleMailParser, MoogleMailSentPage, MoogleMailSentSnapshot,
    MoogleMailSentSummary, MoogleMailSummary,
};
pub use protocol::{
    MoogleMailCookie, MoogleMailEquipmentKey, MoogleMailOperation, MoogleMailTarget,
    MoogleMailToken, MoogleMailWorld,
};
pub use sender::{
    MoogleMailSendOutcome, MoogleMailSendReceipt, MoogleMailSendRequest, MoogleMailSender,
};
pub use transport::{MoogleMailDeliveryTransport, MoogleMailNotificationTemplates};
pub use writer::{MoogleMailComposeState, MoogleMailSendResponseFailure};

/// 将已完成协议校验的 MoogleMail envelope 转换为应用业务 envelope。
///
/// 转换发生在 integrations crate 内部，因此装备 key 只在这里从 MoogleMail 不透明类型
/// 复制到领域 `EquipmentKey`；转换失败时不会把原始 key 或正文放入错误。
pub fn into_application_message(
    mut message: MoogleMailMessage,
    mailbox_key: String,
) -> Result<auction_application::MailMessage, MoogleMailError> {
    if !message.validate() || mailbox_key.trim().is_empty() {
        message.clear_secrets();
        return Err(MoogleMailError::new(
            MoogleMailOperation::Detail,
            message.world,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        ));
    }
    let world_id = message.world.as_str().to_owned();
    let content_digest = message.content_digest();
    let attachments = std::mem::take(&mut message.attachments);
    let mut application_attachments = Vec::with_capacity(attachments.len());
    for mut attachment in attachments {
        let key = match auction_domain::EquipmentKey::new(attachment.key.expose()) {
            Ok(key) => key,
            Err(_) => {
                attachment.key.clear();
                message.clear_secrets();
                return Err(MoogleMailError::new(
                    MoogleMailOperation::Detail,
                    message.world,
                    MoogleMailErrorCategory::ProtocolDrift,
                    false,
                ));
            }
        };
        attachment.key.clear();
        application_attachments.push(auction_application::MailEquipmentAttachment {
            external_equipment_id: attachment.equipment_id,
            key,
        });
    }
    Ok(auction_application::MailMessage {
        mailbox_key,
        external_mail_id: message.external_mail_id,
        world_id,
        received_at: message.received_at,
        sender: message.sender,
        subject: message.subject,
        body: message.body,
        credits: message.credits,
        cod: message.cod,
        attachments: application_attachments,
        returnable: message.returnable,
        claimable: message.claimable,
        content_digest,
    })
}

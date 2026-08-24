//! MoogleMail 远端消息到 Worker poll envelope 的安全字段映射。

use super::super::*;

pub(super) fn map_mooglemail_message(
    message: MoogleMailMessage,
    mailbox_key: String,
) -> Result<MailPollFetchedMessage, auction_integrations::mooglemail::MoogleMailError> {
    let external_mail_id = message.external_mail_id.clone();
    let world_id = message.world.as_str().to_owned();
    let received_at = message.received_at;
    let subject = message.subject.clone();
    let content_digest = message.content_digest();
    let attachment_count = u32::try_from(message.attachments.len()).unwrap_or(u32::MAX);
    let credits = message.credits;
    let cod = message.cod;
    let returnable = message.returnable;
    let claimable = message.claimable;
    let business = into_application_message(message, mailbox_key)?;
    Ok(MailPollFetchedMessage {
        external_mail_id,
        world_id,
        received_at,
        subject,
        content_digest,
        attachment_count,
        credits,
        cod,
        returnable,
        claimable,
        business: Some(business),
    })
}

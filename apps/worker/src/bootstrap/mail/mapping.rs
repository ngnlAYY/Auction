//! MoogleMail 远端消息到 Worker poll envelope 的安全字段映射。

use super::super::*;

use auction_application::MailDisplaySnapshot;

pub(super) fn map_mooglemail_message(
    message: MoogleMailMessage,
    mailbox_key: String,
) -> Result<MailPollFetchedMessage, auction_integrations::mooglemail::MoogleMailError> {
    let external_mail_id = message.external_mail_id.clone();
    let world_id = message.world.as_str().to_owned();
    let received_at = message.received_at;
    let subject = message.subject.clone();
    let content_digest = message.content_digest();
    let attachment_count = u32::try_from(message.attachments.len()).map_err(|_| {
        auction_integrations::mooglemail::MoogleMailError::protocol_drift(
            auction_integrations::mooglemail::MoogleMailOperation::Detail,
            message.world,
        )
    })?;
    let credits = message.credits;
    let cod = message.cod;
    let returnable = message.returnable;
    let claimable = message.claimable;
    let business = into_application_message(message, mailbox_key)?;
    let sender = (!business.sender.is_empty()).then(|| business.sender.clone());
    let display_subject = (!business.subject.is_empty()).then(|| business.subject.clone());
    let headers_present = sender.is_some() && display_subject.is_some();
    let sender = sender.filter(|_| headers_present);
    let message_display =
        headers_present.then(|| MailDisplaySnapshot::from_message(&business)).flatten();
    Ok(MailPollFetchedMessage {
        external_mail_id,
        world_id,
        received_at,
        sender,
        subject,
        message_display,
        content_digest,
        attachment_count,
        credits,
        cod,
        returnable,
        claimable,
        business: Some(business),
    })
}

//! MoogleMail 远端消息到 Worker poll envelope 的安全字段映射。

use auction_application::mail::MailDisplaySnapshot;
use auction_integrations::mooglemail::{MoogleMailPageMessage, into_application_message};
use auction_jobs::{MailPollConsignmentCandidate, MailPollFetchedMessage};
use uuid::Uuid;

pub(super) fn map_mooglemail_message(
    page_message: MoogleMailPageMessage,
    mailbox_key: String,
) -> Result<MailPollFetchedMessage, auction_integrations::mooglemail::MoogleMailError> {
    let (message, consignment_candidate) = page_message.into_parts();
    let consignment_candidate = consignment_candidate.map(map_consignment_candidate).transpose()?;
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
    let headers_present = !business.sender.is_empty() && !business.subject.is_empty();
    let sender = headers_present.then(|| business.sender.clone());
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
        consignment_candidate,
        business: Some(business),
    })
}

fn map_consignment_candidate(
    candidate: auction_integrations::mooglemail::MoogleMailConsignmentCandidate,
) -> Result<MailPollConsignmentCandidate, auction_integrations::mooglemail::MoogleMailError> {
    let binding = candidate.bound.binding().clone();
    let row = candidate.row;
    let observation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "auction/consignment/observation-row/v1/{}/{}/{}/{}",
            binding.authenticated_uid,
            row.world.as_str(),
            row.external_mail_id,
            candidate.bound.remote_observation_id()
        )
        .as_bytes(),
    );
    let observation = candidate.bound.to_envelope_observation(observation_id, None, None);
    let observation_details = candidate.bound.observation();
    Ok(MailPollConsignmentCandidate {
        world_id: row.world.as_str().to_owned(),
        external_mail_id: row.external_mail_id.clone(),
        world_marker: row.world_marker.map(|world| world.as_str().to_owned()),
        sender: observation_details.sender.clone(),
        subject: observation_details.subject.clone(),
        received_minute_raw: row.received_minute_raw.clone(),
        received_at: row.received_at,
        observation,
        binding,
        remote_observation_id: candidate.bound.remote_observation_id(),
        observed_at: candidate.bound.observed_at(),
        subject_digest: observation_details.subject_digest,
        body_digest: observation_details.body_digest,
        raw_count: observation_details.reported_attachment_count,
        legacy_evidence: observation_details.legacy_return_evidence(),
    })
}

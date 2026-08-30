//! MoogleMail Sent 详情及附件/CoD 一致性解析。

use super::super::super::*;

impl MoogleMailParser {
    /// 严格解析与 Sent 摘要身份一致的详情快照。
    pub fn parse_sent_detail(
        &self,
        document: &[u8],
        summary: &MoogleMailSentSummary,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        if !summary.validate() || observed_at < summary.sent_at {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let source = str::from_utf8(document).map_err(|_| {
            detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            )
        })?;
        if source.trim().is_empty() {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let document = Html::parse_document(source);
        parse_token_for_operation(&document, summary.world, MoogleMailOperation::SentDetail)?;
        let envelope_selector =
            selector("#mmail_outer", MoogleMailOperation::SentDetail, summary.world)?;
        let form_selector =
            selector("form#mailform", MoogleMailOperation::SentDetail, summary.world)?;
        let panes_selector =
            selector("#mmail_attachpanes", MoogleMailOperation::SentDetail, summary.world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1 || forms.len() != 1 || panes.len() != 1 {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        if envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let fields =
            parse_detail_fields_for(forms[0], summary.world, MoogleMailOperation::SentDetail)?;
        if !fields.recipient.eq_ignore_ascii_case(&summary.recipient)
            || fields.subject != summary.subject
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::IdentityMismatch,
            ));
        }

        let script = script_text(&document, summary.world, MoogleMailOperation::SentDetail)?;
        let executable = executable_javascript(&script).ok_or_else(|| {
            detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            )
        })?;
        let count = required_script_value(&executable, "attach_count");
        let cod = required_script_value(&executable, "attach_cod");
        let state = required_script_value(&executable, "mail_state");
        if count.is_none()
            || cod.is_none()
            // 当前 Sent 详情使用 mail_state=2；Inbox 独立区分含资产可领取态与空附件已读态。
            || state != Some(2)
            || count.is_some_and(|value| !(0..=MAX_ATTACHMENT_ENTRIES as i64).contains(&value))
            || cod.is_some_and(|value| value < 0)
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (Some(count), Some(cod)) = (count, cod) else {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        };

        let list_selector =
            selector("#mmail_attachlist", MoogleMailOperation::SentDetail, summary.world)?;
        let all_lists = document.select(&list_selector).collect::<Vec<_>>();
        let nested_lists = panes[0].select(&list_selector).collect::<Vec<_>>();
        if all_lists.len() != nested_lists.len() || nested_lists.len() > 1 {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (remaining_equipment_ids, remaining_credits, entries) =
            if let Some(list) = nested_lists.first() {
                let (equipment_ids, credits, entries) =
                    parse_attachment_manifest(*list, summary.world)?;
                (equipment_ids, credits, entries)
            } else {
                (Vec::new(), 0, 0)
            };
        if entries as i64 != count
            || (count == 0 && !nested_lists.is_empty())
            || (count > 0 && nested_lists.len() != 1)
            || (cod > 0 && count == 0)
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }

        let current_cod_selector =
            selector("#mmail_currentcod", MoogleMailOperation::SentDetail, summary.world)?;
        let current_cod = document.select(&current_cod_selector).collect::<Vec<_>>();
        if current_cod.len() > 1
            || current_cod.first().is_some_and(|element| {
                parse_current_cod(&normalize_text(element.text().collect::<String>())) != Some(cod)
            })
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let mut remaining_equipment_ids = remaining_equipment_ids;
        remaining_equipment_ids.sort_unstable();
        let snapshot = MoogleMailSentSnapshot {
            external_mail_id: summary.external_mail_id.clone(),
            world: summary.world,
            recipient: summary.recipient.clone(),
            subject: summary.subject.clone(),
            sent_at: summary.sent_at,
            remaining_credits,
            cod_pending: cod,
            collected: count == 0 && cod == 0,
            remaining_equipment_ids,
            observed_at,
        };
        if !snapshot.validate() {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        Ok(snapshot)
    }
}

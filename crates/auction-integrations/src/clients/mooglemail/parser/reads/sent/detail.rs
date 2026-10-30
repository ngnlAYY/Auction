//! MoogleMail Sent 详情及正文/附件一致性解析。

use super::super::super::*;

impl MoogleMailParser {
    /// 解析 Sent 详情中的正文摘要与当前资产，不依赖远端 sent_at 的时区归属。
    pub(crate) fn parse_sent_content_proof(
        &self,
        document: &[u8],
        summary: &MoogleMailSentSummary,
    ) -> Result<MoogleMailSentContentProof, MoogleMailError> {
        if !summary.validate() {
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
        let operation = MoogleMailOperation::SentDetail;
        parse_token_for_operation(&document, summary.world, operation)?;
        let envelope_selector = selector("#mmail_outer", operation, summary.world)?;
        let form_selector = selector("form#mailform", operation, summary.world)?;
        let panes_selector = selector("#mmail_attachpanes", operation, summary.world)?;
        let envelope =
            unique_element(document.select(&envelope_selector), operation, summary.world)?;
        let form = unique_element(document.select(&form_selector), operation, summary.world)?;
        let pane = unique_element(document.select(&panes_selector), operation, summary.world)?;
        if envelope.select(&form_selector).take(2).count() != 1
            || envelope.select(&panes_selector).take(2).count() != 1
        {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let fields = parse_detail_fields_for(form, summary.world, operation)?;
        // 列表页标题会被服务端截断；详情必须保留完整标题，列表标题只能作为前缀匹配。
        if !fields.recipient.eq_ignore_ascii_case(&summary.recipient)
            || !subject_is_list_projection(&summary.subject, &fields.subject)
        {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::IdentityMismatch,
            ));
        }

        let textarea_selector = selector("textarea", operation, summary.world)?;
        let textarea = unique_element(form.select(&textarea_selector), operation, summary.world)?;
        let mut body = textarea.text().collect::<String>();
        if !valid_message_body(&body) {
            clear_sent_body(&mut body);
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let body_digest = canonical_mail_body_digest(&body);
        clear_sent_body(&mut body);

        let script = script_text(&document, summary.world, operation)?;
        let executable = executable_javascript(&script).ok_or_else(|| {
            detail_error_for(operation, summary.world, MoogleMailErrorCategory::ProtocolDrift)
        })?;
        let count = required_script_value(&executable, "attach_count");
        let cod = required_script_value(&executable, "attach_cod");
        let state = required_script_value(&executable, "mail_state");
        // Sent 详情与 Inbox 共用状态位：1 表示仍有待领取资产，2 表示已领取完毕；
        // 两者都必须接受，否则仍有待领取资产（如带 COD）的已发送邮件无法证明。
        let collected_state = state == Some(2);
        if count.is_none()
            || cod.is_none()
            || !matches!(state, Some(1 | 2))
            || (collected_state && (count != Some(0) || cod != Some(0)))
            || count.is_some_and(|value| !(0..=MAX_ATTACHMENT_ENTRIES as i64).contains(&value))
            || cod.is_some_and(|value| value < 0)
        {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (Some(count), Some(cod)) = (count, cod) else {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        };

        let list_selector = selector("#mmail_attachlist", operation, summary.world)?;
        let all_lists = document.select(&list_selector).take(2).collect::<Vec<_>>();
        let nested_lists = pane.select(&list_selector).take(2).collect::<Vec<_>>();
        if all_lists.len() != nested_lists.len() || nested_lists.len() > 1 {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (mut remaining_equipment_ids, remaining_credits, entries) =
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
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let current_cod_selector = selector("#mmail_currentcod", operation, summary.world)?;
        let current_cod = document.select(&current_cod_selector).take(2).collect::<Vec<_>>();
        if current_cod.len() > 1
            || current_cod.first().is_some_and(|element| {
                parse_current_cod(&normalize_text(element.text().collect::<String>())) != Some(cod)
            })
        {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        remaining_equipment_ids.sort_unstable();
        let proof = MoogleMailSentContentProof {
            external_mail_id: summary.external_mail_id.clone(),
            world: summary.world,
            recipient: summary.recipient.clone(),
            subject: fields.subject,
            body_digest,
            remaining_credits,
            cod_pending: cod,
            remaining_equipment_ids,
        };
        if !proof.validate() {
            return Err(detail_error_for(
                operation,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        Ok(proof)
    }

    /// 严格解析与 Sent 摘要身份一致的详情快照，并保留旧的时间契约。
    pub fn parse_sent_detail(
        &self,
        document: &[u8],
        summary: &MoogleMailSentSummary,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        // 列表时间按页面 UTC 文本解析，只剩分钟级取整误差；发送先后仍由
        // claim.claimed_at 与 observed_at 把关，出现偏差异常时保留证据但不拒绝整封邮件。
        if observed_at < summary.sent_at {
            tracing::warn!(
                world = %summary.world,
                sent_at = %summary.sent_at,
                observed_at = %observed_at,
                "MoogleMail Sent 详情时间晚于本地观测；按弱时间语义继续解析"
            );
        }
        let proof = self.parse_sent_content_proof(document, summary)?;
        let snapshot = MoogleMailSentSnapshot {
            external_mail_id: proof.external_mail_id,
            world: proof.world,
            recipient: proof.recipient,
            subject: proof.subject,
            sent_at: summary.sent_at,
            remaining_credits: proof.remaining_credits,
            cod_pending: proof.cod_pending,
            collected: proof.remaining_credits == 0
                && proof.cod_pending == 0
                && proof.remaining_equipment_ids.is_empty(),
            remaining_equipment_ids: proof.remaining_equipment_ids,
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

fn clear_sent_body(body: &mut String) {
    let mut bytes = std::mem::take(body).into_bytes();
    bytes.fill(0);
}

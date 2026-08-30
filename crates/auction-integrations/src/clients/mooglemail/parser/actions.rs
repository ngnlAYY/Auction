use super::*;

impl MoogleMailParser {
    /// 严格解析单封详情，并以收件箱摘要校验稳定身份。
    pub fn parse_detail(
        &self,
        document: &[u8],
        summary: &MoogleMailSummary,
    ) -> Result<MoogleMailMessage, MoogleMailError> {
        if !summary.validate() {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::Configuration));
        }
        let source = str::from_utf8(document)
            .map_err(|_| detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let document = Html::parse_document(source);
        parse_token(&document, summary.world)?;
        if has_result_message(&document, summary.world)? {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }

        let envelope_selector =
            selector("#mmail_outer", MoogleMailOperation::Detail, summary.world)?;
        let form_selector = selector("form#mailform", MoogleMailOperation::Detail, summary.world)?;
        let panes_selector =
            selector("#mmail_attachpanes", MoogleMailOperation::Detail, summary.world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1 || forms.len() != 1 || panes.len() != 1 {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let fields = parse_detail_fields(forms[0], summary.world)?;
        if !fields.sender.eq_ignore_ascii_case(&summary.sender) {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::IdentityMismatch));
        }
        let textarea_selector = selector("textarea", MoogleMailOperation::Detail, summary.world)?;
        let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
        if textareas.len() != 1 {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let body = textareas[0].text().collect::<String>();
        if !valid_message_body(&body) {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let assets = parse_detail_assets(&document, &body, summary.world)?;
        let (returnable, claimable) = parse_detail_actions(envelopes[0], summary.world)?;
        let has_assets = !assets.attachments.is_empty() || assets.credits > 0;
        if claimable != has_assets {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let message = MoogleMailMessage {
            external_mail_id: summary.external_mail_id.clone(),
            world: summary.world,
            received_at: summary.received_at,
            sender: fields.sender,
            subject: fields.subject,
            body,
            credits: assets.credits,
            cod: assets.cod,
            attachments: assets.attachments,
            returnable,
            claimable,
        };
        if !message.validate() {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(message)
    }

    /// 严格解析详情页中可执行的 return/take 动作状态。
    pub fn parse_action_state(
        &self,
        source_bytes: &[u8],
        world: MoogleMailWorld,
    ) -> Result<MoogleMailActionState, MoogleMailError> {
        let source = str::from_utf8(source_bytes)
            .map_err(|_| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let document = Html::parse_document(source);
        if has_result_message(&document, world)? {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let token = parse_token(&document, world)?;
        let envelope_selector = selector("#mmail_outer", MoogleMailOperation::Detail, world)?;
        let form_selector = selector("form#mailform", MoogleMailOperation::Detail, world)?;
        let panes_selector = selector("#mmail_attachpanes", MoogleMailOperation::Detail, world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1
            || forms.len() != 1
            || panes.len() != 1
            || envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let fields = parse_detail_fields(forms[0], world)?;
        let textarea_selector = selector("textarea", MoogleMailOperation::Detail, world)?;
        let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
        if textareas.len() != 1 {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let body = textareas[0].text().collect::<String>();
        if !valid_message_body(&body) {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let assets = parse_detail_asset_manifest(&document, world)?;
        let (returnable, claimable) = parse_detail_actions(envelopes[0], world)?;
        let has_assets = !assets.equipment_ids.is_empty() || assets.credits > 0;
        if claimable != has_assets {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(MoogleMailActionState {
            token,
            recipient: fields.recipient,
            sender: fields.sender,
            subject: fields.subject,
            body,
            equipment_ids: assets.equipment_ids,
            credits: assets.credits,
            cod: assets.cod,
            attachment_count: assets.entry_count,
            returnable,
            claimable,
        })
    }

    /// 通过当前只读 `[RTS]` 回执或旧版 Inbox 中目标 `mid` 缺席证明整封退回已完成。
    pub fn prove_return_completed(
        &self,
        before: &MoogleMailActionState,
        source_bytes: &[u8],
        world: MoogleMailWorld,
        external_mail_id: &str,
    ) -> Result<(), MoogleMailError> {
        let operation = MoogleMailOperation::ReturnMessage;
        if !valid_opaque_id(external_mail_id) {
            return Err(action_proof_error(operation, world));
        }
        let source =
            str::from_utf8(source_bytes).map_err(|_| action_proof_error(operation, world))?;
        if source.trim().is_empty() {
            return Err(action_proof_error(operation, world));
        }
        let document = Html::parse_document(source);
        if has_result_message(&document, world).map_err(|_| action_proof_error(operation, world))? {
            return Err(action_proof_error(operation, world));
        }
        if returned_receipt_matches(&document, before, world)
            .map_err(|_| action_proof_error(operation, world))?
        {
            return Ok(());
        }
        let form_selector = selector("form#mailform", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let table_selector = selector("table#mmail_list", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let reference_selector = selector("[href], [onclick], [action]", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let tables = document.select(&table_selector).collect::<Vec<_>>();
        if document.select(&form_selector).next().is_some()
            || tables.len() != 1
            || document.select(&reference_selector).any(|element| {
                ["href", "onclick", "action"].iter().any(|attribute| {
                    element
                        .value()
                        .attr(attribute)
                        .is_some_and(|value| attribute_references_mail_id(value, external_mail_id))
                })
            })
        {
            return Err(action_proof_error(operation, world));
        }
        let row_selector =
            selector("tr", operation, world).map_err(|_| action_proof_error(operation, world))?;
        let cell_selector =
            selector("td", operation, world).map_err(|_| action_proof_error(operation, world))?;
        let empty_selector = selector("td#mmail_nnm", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
        let rows = tables[0].select(&row_selector).collect::<Vec<_>>();
        if rows.len() < 2
            || !valid_inbox_header(rows[0], world)
                .map_err(|_| action_proof_error(operation, world))?
        {
            return Err(action_proof_error(operation, world));
        }
        for row in &rows[1..] {
            let cells = row.select(&cell_selector).count();
            if row.select(&empty_selector).count() == 1 && cells == 1 {
                continue;
            }
            let mail_id = row.value().attr("onclick").and_then(extract_mid);
            if cells != 4 || mail_id.as_deref().is_none_or(|mail_id| mail_id == external_mail_id) {
                return Err(action_proof_error(operation, world));
            }
        }
        Ok(())
    }

    /// 通过动作前后的邮件身份、资产归零和正文回执证明附件领取已完成。
    pub fn prove_take_completed(
        &self,
        before: &MoogleMailActionState,
        source_bytes: &[u8],
        world: MoogleMailWorld,
    ) -> Result<(), MoogleMailError> {
        let operation = MoogleMailOperation::Take;
        match self.parse_action_state(source_bytes, world) {
            Ok(after) => {
                if after.recipient != before.recipient
                    || after.sender != before.sender
                    || after.subject != before.subject
                    || !after.equipment_ids.is_empty()
                    || after.credits != 0
                    || after.cod != 0
                    || after.attachment_count != 0
                    || after.claimable
                {
                    return Err(action_proof_error(operation, world));
                }
                prove_take_identity_and_receipts(
                    before,
                    &after.recipient,
                    &after.sender,
                    &after.subject,
                    &after.body,
                    world,
                )
            }
            Err(_) => {
                let (fields, body) = parse_terminal_empty_detail(source_bytes, world)?;
                prove_take_identity_and_receipts(
                    before,
                    &fields.recipient,
                    &fields.sender,
                    &fields.subject,
                    &body,
                    world,
                )
            }
        }
    }
}

fn returned_receipt_matches(
    document: &Html,
    before: &MoogleMailActionState,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    if !before.returnable {
        return Ok(false);
    }
    let envelope_selector = selector("#mmail_outer", MoogleMailOperation::ReturnMessage, world)?;
    let form_selector = selector("form#mailform", MoogleMailOperation::ReturnMessage, world)?;
    let panes_selector = selector("#mmail_attachpanes", MoogleMailOperation::ReturnMessage, world)?;
    let textarea_selector = selector("textarea", MoogleMailOperation::ReturnMessage, world)?;
    let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
    let forms = document.select(&form_selector).collect::<Vec<_>>();
    let panes = document.select(&panes_selector).collect::<Vec<_>>();
    if envelopes.len() != 1
        || forms.len() != 1
        || panes.len() != 1
        || envelopes[0].select(&form_selector).count() != 1
        || envelopes[0].select(&panes_selector).count() != 1
    {
        return Ok(false);
    }
    parse_token(document, world)?;
    let fields = parse_detail_fields(forms[0], world)?;
    let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
    if textareas.len() != 1 {
        return Ok(false);
    }
    let body = textareas[0].text().collect::<String>();
    if !valid_message_body(&body) {
        return Ok(false);
    }
    let assets = parse_detail_asset_manifest(document, world)?;
    let (returnable, claimable) = parse_detail_actions(envelopes[0], world)?;
    let mut actual_equipment_ids = assets.equipment_ids;
    let mut expected_equipment_ids = before.equipment_ids.clone();
    actual_equipment_ids.sort_unstable();
    expected_equipment_ids.sort_unstable();
    let expected_subject = format!("[RTS] {}", before.subject);
    let expected_body =
        format!("{}\n\nThis message was returned from {}, kupo!", before.body, before.recipient);
    Ok(!returnable
        && !claimable
        && fields.recipient.eq_ignore_ascii_case(&before.sender)
        && fields.sender == "MoogleMail"
        && fields.subject == expected_subject
        && body == expected_body
        && assets.credits == before.credits
        && assets.cod == before.cod
        && assets.entry_count == before.attachment_count
        && actual_equipment_ids == expected_equipment_ids)
}

use super::*;

impl MoogleMailParser {
    /// 严格解析单封详情；摘要只提供 MID、world 与收件时间，完整发件人只采信详情。
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
        // 列表显示名可能被服务端截断，不能据此拒绝详情中的完整发件人或重建业务身份。
        let fields = parse_detail_fields(forms[0], summary.world)?;
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

    /// 只通过身份、正文和资产精确匹配的 `[RTS]` 回执证明整封退回已完成。
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
        // Inbox 只列未读邮件，写前详情 GET 本身就会使目标消失；缺席绝不是退回证明。
        Err(action_proof_error(operation, world))
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
    // 退信证明只比较原资产和正文，不复制装备 ID 或整段可能含密钥的正文。
    let mut actual_equipment_ids = assets.equipment_ids.iter().collect::<Vec<_>>();
    let mut expected_equipment_ids = before.equipment_ids.iter().collect::<Vec<_>>();
    actual_equipment_ids.sort_unstable();
    expected_equipment_ids.sort_unstable();
    let body_matches = body
        .strip_prefix(before.body.as_str())
        .and_then(|suffix| suffix.strip_prefix("\n\nThis message was returned from "))
        .and_then(|suffix| suffix.strip_prefix(before.recipient.as_str()))
        == Some(", kupo!");
    Ok(!returnable
        && !claimable
        && fields.recipient.eq_ignore_ascii_case(&before.sender)
        && fields.sender == "MoogleMail"
        && fields.subject.strip_prefix("[RTS] ") == Some(before.subject.as_str())
        && body_matches
        && assets.credits == before.credits
        && assets.cod == before.cod
        && assets.entry_count == before.attachment_count
        && actual_equipment_ids == expected_equipment_ids)
}

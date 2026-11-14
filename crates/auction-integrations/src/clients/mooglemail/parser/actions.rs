//! MoogleMail 详情页与动作结果的严格解析。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use scraper::Html;

use super::{
    MoogleMailActionState, MoogleMailMessage, MoogleMailParser, MoogleMailSummary,
    action_proof_error, detail_error, has_result_message, parse_detail_actions,
    parse_detail_asset_manifest, parse_detail_assets, parse_detail_fields,
    parse_terminal_empty_detail, parse_token, prove_take_identity_and_receipts, selector,
    unique_element, valid_message_body,
};
use crate::clients::mooglemail::protocol::valid_opaque_id;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};
use crate::safe_diagnostics;

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
        let source = str::from_utf8(document).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "decode_message_detail",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(summary.world.as_str()),
                    ..Default::default()
                },
            );
            detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift)
        })?;
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
        let operation = MoogleMailOperation::Detail;
        let envelope =
            unique_element(document.select(&envelope_selector), operation, summary.world)?;
        let form = unique_element(document.select(&form_selector), operation, summary.world)?;
        unique_element(document.select(&panes_selector), operation, summary.world)?;
        if envelope.select(&form_selector).take(2).count() != 1
            || envelope.select(&panes_selector).take(2).count() != 1
        {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        // 列表显示名可能被服务端截断，不能据此拒绝详情中的完整发件人或重建业务身份。
        let fields = parse_detail_fields(form, summary.world)?;
        let textarea_selector = selector("textarea", MoogleMailOperation::Detail, summary.world)?;
        let textarea = unique_element(form.select(&textarea_selector), operation, summary.world)?;
        let body = textarea.text().collect::<String>();
        if !valid_message_body(&body) {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let assets = parse_detail_assets(&document, &body, summary.world)?;
        let (returnable, claimable) = parse_detail_actions(envelope, summary.world)?;
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
        let source = str::from_utf8(source_bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "decode_action_state",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            detail_error(world, MoogleMailErrorCategory::ProtocolDrift)
        })?;
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
        let operation = MoogleMailOperation::Detail;
        let envelope = unique_element(document.select(&envelope_selector), operation, world)?;
        let form = unique_element(document.select(&form_selector), operation, world)?;
        unique_element(document.select(&panes_selector), operation, world)?;
        if envelope.select(&form_selector).take(2).count() != 1
            || envelope.select(&panes_selector).take(2).count() != 1
        {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let fields = parse_detail_fields(form, world)?;
        let textarea_selector = selector("textarea", MoogleMailOperation::Detail, world)?;
        let textarea = unique_element(form.select(&textarea_selector), operation, world)?;
        let body = textarea.text().collect::<String>();
        if !valid_message_body(&body) {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let assets = parse_detail_asset_manifest(&document, world)?;
        let (returnable, claimable) = parse_detail_actions(envelope, world)?;
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
        let source = str::from_utf8(source_bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_return",
                "decode_return_receipt",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })?;
        if source.trim().is_empty() {
            return Err(action_proof_error(operation, world));
        }
        let document = Html::parse_document(source);
        if has_result_message(&document, world).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_return",
                "verify_result_message",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })? {
            return Err(action_proof_error(operation, world));
        }
        if returned_receipt_matches(&document, before, world).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_return",
                "verify_return_receipt",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })? {
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
            Err(diagnostic_cause) => {
                safe_diagnostics::debug(
                    "mooglemail_take",
                    "parse_terminal_detail",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );

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
    let operation = MoogleMailOperation::ReturnMessage;
    let (Ok(envelope), Ok(form), Ok(_)) = (
        unique_element(document.select(&envelope_selector), operation, world),
        unique_element(document.select(&form_selector), operation, world),
        unique_element(document.select(&panes_selector), operation, world),
    ) else {
        return Ok(false);
    };
    if envelope.select(&form_selector).take(2).count() != 1
        || envelope.select(&panes_selector).take(2).count() != 1
    {
        return Ok(false);
    }
    parse_token(document, world)?;
    let fields = parse_detail_fields(form, world)?;
    let Ok(textarea) = unique_element(form.select(&textarea_selector), operation, world) else {
        return Ok(false);
    };
    let body = textarea.text().collect::<String>();
    if !valid_message_body(&body) {
        return Ok(false);
    }
    let assets = parse_detail_asset_manifest(document, world)?;
    let (returnable, claimable) = parse_detail_actions(envelope, world)?;
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

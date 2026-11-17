//! MoogleMail 详情页与动作结果的严格解析。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::{
    MoogleMailboxInboxAssets, MoogleMailboxReturnTake, MoogleMailboxReturnTakeIdentity,
};
use scraper::Html;

use super::{
    MoogleMailActionState, MoogleMailMessage, MoogleMailParser, MoogleMailSummary,
    action_proof_error, detail_error, has_result_message, parse_detail_actions,
    parse_detail_asset_manifest, parse_detail_assets, parse_detail_fields,
    parse_terminal_empty_detail, parse_token, prove_returned_take_receipts,
    prove_take_identity_and_receipts, read_selector, unique_element, valid_message_body,
};
use crate::clients::mooglemail::protocol::valid_opaque_id;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};
use crate::safe_diagnostics;

impl MoogleMailParser {
    /// 严格解析单封详情；摘要只提供 MID、world 与收件时间，完整发件人只采信详情。
    /// 这三项身份字段直接取自摘要，调用方据此省去逐字段复核；改为从页面读取时须恢复复核。
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
            read_selector!("#mmail_outer", MoogleMailOperation::Detail, summary.world)?;
        let form_selector =
            read_selector!("form#mailform", MoogleMailOperation::Detail, summary.world)?;
        let panes_selector =
            read_selector!("#mmail_attachpanes", MoogleMailOperation::Detail, summary.world)?;
        let operation = MoogleMailOperation::Detail;
        let envelope =
            unique_element(document.select(envelope_selector), operation, summary.world)?;
        let form = unique_element(document.select(form_selector), operation, summary.world)?;
        unique_element(document.select(panes_selector), operation, summary.world)?;
        if envelope.select(form_selector).take(2).count() != 1
            || envelope.select(panes_selector).take(2).count() != 1
        {
            return Err(detail_error(summary.world, MoogleMailErrorCategory::ProtocolDrift));
        }
        // 列表显示名可能被服务端截断，不能据此拒绝详情中的完整发件人或重建业务身份。
        let fields = parse_detail_fields(form, summary.world)?;
        let textarea_selector =
            read_selector!("textarea", MoogleMailOperation::Detail, summary.world)?;
        let textarea = unique_element(form.select(textarea_selector), operation, summary.world)?;
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
        let envelope_selector = read_selector!("#mmail_outer", MoogleMailOperation::Detail, world)?;
        let form_selector = read_selector!("form#mailform", MoogleMailOperation::Detail, world)?;
        let panes_selector =
            read_selector!("#mmail_attachpanes", MoogleMailOperation::Detail, world)?;
        let operation = MoogleMailOperation::Detail;
        let envelope = unique_element(document.select(envelope_selector), operation, world)?;
        let form = unique_element(document.select(form_selector), operation, world)?;
        unique_element(document.select(panes_selector), operation, world)?;
        if envelope.select(form_selector).take(2).count() != 1
            || envelope.select(panes_selector).take(2).count() != 1
        {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let fields = parse_detail_fields(form, world)?;
        let textarea_selector = read_selector!("textarea", MoogleMailOperation::Detail, world)?;
        let textarea = unique_element(form.select(textarea_selector), operation, world)?;
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

    /// 结算退件 Take 原 MID 详情页的只读结论：远端明确答复不存在为 `Missing`；仍挂资产为
    /// `Present`；附件已空时只有原身份、原正文前缀与领取回执都成立才是 `Taken`，否则 `Unproven`。
    pub fn returned_take_observation(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        identity: &MoogleMailboxReturnTakeIdentity,
    ) -> Result<MoogleMailboxReturnTake, MoogleMailError> {
        Ok(match self.parse_action_state_or_missing(document, world)? {
            None => MoogleMailboxReturnTake::Missing,
            Some(state) => returned_take_state(&state, identity),
        })
    }
}

/// 已解析详情页对结算退件 Take 的结论；仍可领取或仍有任何资产时交给调用方比对是否完整，并带出
/// 页面是否仍给出领取入口。
pub(crate) fn returned_take_state(
    state: &MoogleMailActionState,
    identity: &MoogleMailboxReturnTakeIdentity,
) -> MoogleMailboxReturnTake {
    if state.claimable
        || !state.equipment_ids.is_empty()
        || state.credits != 0
        || state.cod != 0
        || state.attachment_count != 0
    {
        let mut equipment_ids = state.equipment_ids.clone();
        equipment_ids.sort_unstable();
        return MoogleMailboxReturnTake::Present {
            assets: MoogleMailboxInboxAssets {
                sender: state.sender.clone(),
                equipment_ids,
                credits: state.credits,
                cod: state.cod,
            },
            claimable: state.claimable,
        };
    }
    if prove_returned_take_receipts(state, identity) {
        MoogleMailboxReturnTake::Taken
    } else {
        MoogleMailboxReturnTake::Unproven
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
    let envelope_selector =
        read_selector!("#mmail_outer", MoogleMailOperation::ReturnMessage, world)?;
    let form_selector = read_selector!("form#mailform", MoogleMailOperation::ReturnMessage, world)?;
    let panes_selector =
        read_selector!("#mmail_attachpanes", MoogleMailOperation::ReturnMessage, world)?;
    let textarea_selector = read_selector!("textarea", MoogleMailOperation::ReturnMessage, world)?;
    let operation = MoogleMailOperation::ReturnMessage;
    let (Ok(envelope), Ok(form), Ok(_)) = (
        unique_element(document.select(envelope_selector), operation, world),
        unique_element(document.select(form_selector), operation, world),
        unique_element(document.select(panes_selector), operation, world),
    ) else {
        return Ok(false);
    };
    if envelope.select(form_selector).take(2).count() != 1
        || envelope.select(panes_selector).take(2).count() != 1
    {
        return Ok(false);
    }
    parse_token(document, world)?;
    let fields = parse_detail_fields(form, world)?;
    let Ok(textarea) = unique_element(form.select(textarea_selector), operation, world) else {
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

#[cfg(test)]
mod tests {
    use auction_application::mail::{MoogleMailboxReturnTake, MoogleMailboxReturnTakeIdentity};

    use super::returned_take_state;
    use crate::clients::mooglemail::MoogleMailWorld;
    use crate::clients::mooglemail::parser::{MoogleMailParser, canonical_mail_body_digest};

    /// 真实已领取详情页（taken-3125358）：只替换发件人、主题与正文，页面结构、资产脚本与按钮不变。
    const TAKEN: &str = include_str!("../../../../tests/fixtures/mooglemail/taken-3125358.html");
    const MISSING: &str =
        include_str!("../../../../tests/fixtures/mooglemail/inboxdetail-missing.html");
    const SUBJECT: &str = "[RTS] Auction seller payout [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]";
    const ORIGINAL: &str = "Your auction payout is attached.\n\nAmount: 90 Credits\n\nReference: 5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
    const SUFFIX: &str = "\n\nThis message was returned from Seller Name, kupo!";

    /// 脱敏页面里 Bot 账号（退件收件人）的名称。
    const BOT: &str = "Ujlhovl";

    fn identity() -> MoogleMailboxReturnTakeIdentity {
        MoogleMailboxReturnTakeIdentity {
            sender: "MoogleMail".to_owned(),
            subject: SUBJECT.to_owned(),
            original_body_digest: canonical_mail_body_digest(ORIGINAL),
            returned_from: "Seller Name".to_owned(),
            cod: 0,
            attachment_count: 1,
            expected_recipient: Some(BOT.to_owned()),
        }
    }

    fn page(sender: &str, subject: &str, body: &str) -> String {
        let start = TAKEN.find("<textarea").expect("正文开始");
        let open_end = start + TAKEN[start..].find('>').expect("正文标签结束") + 1;
        let close = TAKEN.find("</textarea>").expect("正文结束");
        let page = format!("{}{body}{}", &TAKEN[..open_end], &TAKEN[close..]);
        page.replacen("value=\"Pnfcgarl\"", &format!("value=\"{sender}\""), 1).replacen(
            "value=\"auction\"",
            &format!("value=\"{subject}\""),
            1,
        )
    }

    fn observe(html: &str, identity: &MoogleMailboxReturnTakeIdentity) -> MoogleMailboxReturnTake {
        MoogleMailParser
            .returned_take_observation(html.as_bytes(), MoogleMailWorld::Main, identity)
            .expect("真实页面结构可解析")
    }

    fn taken_body() -> String {
        format!("{ORIGINAL}{SUFFIX}\n\nRemoved attachment: 90 Credits")
    }

    /// 原身份、原退件正文前缀与逐条领取回执都成立才是领取完成证明；CRLF 不改变结论。
    #[test]
    fn returned_take_receipt_proves_completion() {
        let identity = identity();
        assert_eq!(
            observe(&page("MoogleMail", SUBJECT, &taken_body()), &identity),
            MoogleMailboxReturnTake::Taken
        );
        let crlf = taken_body().replace('\n', "\r\n");
        assert_eq!(
            observe(&page("MoogleMail", SUBJECT, &crlf), &identity),
            MoogleMailboxReturnTake::Taken
        );
        assert_eq!(observe(MISSING, &identity), MoogleMailboxReturnTake::Missing);
    }

    /// 附件已为空但回执不成立、回执行数或 COD 回执不符、原正文或原收件人不符、发件人或主题不符，
    /// 都只是未证明，不能当作领取完成。
    #[test]
    fn empty_attachments_without_matching_receipt_stay_unproven() {
        let identity = identity();
        let unproven = |html: String, identity: &MoogleMailboxReturnTakeIdentity| {
            assert_eq!(observe(&html, identity), MoogleMailboxReturnTake::Unproven, "{html:.0}");
        };
        unproven(page("MoogleMail", SUBJECT, &format!("{ORIGINAL}{SUFFIX}")), &identity);
        unproven(page("MoogleMail", SUBJECT, ORIGINAL), &identity);
        let mut two = identity.clone();
        two.attachment_count = 2;
        unproven(page("MoogleMail", SUBJECT, &taken_body()), &two);
        unproven(
            page("MoogleMail", SUBJECT, &format!("{}\nCoD Paid: 10 Credits", taken_body())),
            &identity,
        );
        unproven(
            page("MoogleMail", SUBJECT, &taken_body().replace("Amount: 90", "Amount: 91")),
            &identity,
        );
        unproven(
            page("MoogleMail", SUBJECT, &taken_body().replace("Seller Name", "Other Name")),
            &identity,
        );
        unproven(page("BuyerPlayer", SUBJECT, &taken_body()), &identity);
        unproven(page("MoogleMail", "[RTS] Auction buyer refund [x]", &taken_body()), &identity);
    }

    /// 只有页面收件人改变（发件人、主题、原正文前缀与领取回执全都成立）也不是领取完成：收件人必须
    /// 等于执行 Take 的凭据所属账号的预期玩家名；凭据没有预期名称时无法比较，同样只是未证明。
    #[test]
    fn returned_take_receipt_requires_the_take_account_as_recipient() {
        let taken = page("MoogleMail", SUBJECT, &taken_body());
        assert_eq!(observe(&taken, &identity()), MoogleMailboxReturnTake::Taken);
        let other_recipient = taken.replacen(&format!("value=\"{BOT}\""), "value=\"Other Bot\"", 1);
        assert_ne!(other_recipient, taken, "页面里必须有可替换的收件人字段");
        assert_eq!(observe(&other_recipient, &identity()), MoogleMailboxReturnTake::Unproven);
        let mut expected_other = identity();
        expected_other.expected_recipient = Some("Other Bot".to_owned());
        assert_eq!(observe(&taken, &expected_other), MoogleMailboxReturnTake::Unproven);
        let mut incomparable = identity();
        incomparable.expected_recipient = None;
        assert_eq!(observe(&taken, &incomparable), MoogleMailboxReturnTake::Unproven);
    }

    /// 仍可领取或仍挂资产时交给调用方比对完整性，不判领取完成；同时带出页面是否仍给出领取入口
    /// （再次领回的证明只认可领取的完整资产）。
    #[test]
    fn remaining_assets_are_reported_as_present() {
        let mut state = MoogleMailParser
            .parse_action_state(
                page("MoogleMail", SUBJECT, &taken_body()).as_bytes(),
                MoogleMailWorld::Main,
            )
            .expect("真实页面结构可解析");
        state.credits = 90;
        state.attachment_count = 1;
        state.claimable = true;
        let MoogleMailboxReturnTake::Present { assets, claimable } =
            returned_take_state(&state, &identity())
        else {
            panic!("仍挂资产必须是 Present");
        };
        assert!(claimable);
        assert_eq!((assets.sender.as_str(), assets.credits, assets.cod), ("MoogleMail", 90, 0));
        assert!(assets.equipment_ids.is_empty());
        state.claimable = false;
        assert!(
            matches!(
                returned_take_state(&state, &identity()),
                MoogleMailboxReturnTake::Present { claimable: false, .. }
            ),
            "仍挂资产但没有领取入口时如实报告不可领取"
        );
    }
}

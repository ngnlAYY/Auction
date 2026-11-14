//! 详情页解析的字段、附件与终态证明辅助。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::safe_diagnostics;

use super::super::*;

pub(crate) struct DetailFields {
    pub(crate) recipient: String,
    pub(crate) sender: String,
    pub(crate) subject: String,
}

pub(crate) struct DetailAssets {
    pub(crate) attachments: Vec<MoogleMailAttachment>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
}

pub(crate) struct DetailAssetManifest {
    pub(crate) equipment_ids: Vec<String>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) entry_count: usize,
}

pub(crate) fn parse_terminal_empty_detail(
    source_bytes: &[u8],
    world: MoogleMailWorld,
) -> Result<(DetailFields, String), MoogleMailError> {
    let operation = MoogleMailOperation::Take;
    let source = str::from_utf8(source_bytes).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "mooglemail_take",
            "decode_terminal_detail",
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
            "mooglemail_take",
            "verify_terminal_result",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })? {
        return Err(action_proof_error(operation, world));
    }
    parse_token_for_operation(&document, world, operation).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "mooglemail_take",
            "verify_terminal_token",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;

    let envelope_selector =
        selector("#mmail_outer", operation, world).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_take",
                "build_terminal_envelope_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })?;
    let form_selector =
        selector("form#mailform", operation, world).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_take",
                "build_terminal_form_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })?;
    let all_form_selector = selector("form", operation, world).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_take",
            "build_terminal_all_forms_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;
    let panes_selector =
        selector("#mmail_attachpanes", operation, world).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_take",
                "build_terminal_panes_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })?;
    let buttons_selector =
        selector("#mmail_showbuttons", operation, world).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_take",
                "build_terminal_buttons_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })?;
    let envelope = unique_element(document.select(&envelope_selector), operation, world).map_err(
        |diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_take",
                "validate_terminal_envelope",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        },
    )?;
    let form = unique_element(document.select(&form_selector), operation, world).map_err(
        |diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_take",
                "validate_terminal_form",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        },
    )?;
    unique_element(document.select(&panes_selector), operation, world).map_err(
        |diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_take",
                "validate_terminal_panes",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        },
    )?;
    unique_element(document.select(&buttons_selector), operation, world).map_err(
        |diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_take",
                "validate_terminal_buttons",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        },
    )?;
    // 侧栏 stamina 的 `#recoverform` 出现在每个真实页面上，表单唯一性只能在 envelope 内核对。
    if envelope.select(&all_form_selector).take(2).count() != 1
        || envelope.select(&form_selector).take(2).count() != 1
        || envelope.select(&panes_selector).take(2).count() != 1
        || envelope.select(&buttons_selector).take(2).count() != 1
    {
        return Err(action_proof_error(operation, world));
    }

    // 导航栏与提示区不属于邮件状态；只在邮件 envelope 内核对剩余动作与资产。
    let onclick_selector = selector("[onclick]", operation, world).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_take",
            "build_terminal_action_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;
    let asset_selector = selector(
        "#mmail_attachlist, #mmail_attachcount, #newcod",
        operation,
        world,
    )
    .map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_take",
            "build_terminal_asset_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;
    let equipment_selector =
        selector("[onmouseover]", operation, world).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_take",
                "build_terminal_equipment_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        })?;
    if envelope.select(&asset_selector).next().is_some()
        || envelope.select(&equipment_selector).next().is_some()
        || envelope.select(&onclick_selector).any(|action| {
            // 空终态页唯一允许的剩余动作是“回复”：它只打开 compose，不触碰邮件与资产。
            !is_reply_action(action.value().attr("onclick").unwrap_or_default(), world)
        })
    {
        return Err(action_proof_error(operation, world));
    }

    // 用户正文和 HTML 注释不属于协议状态；仅在脚本与可执行属性中保守检查原始标记。
    for element in document.root_element().descendants().filter_map(ElementRef::wrap) {
        if element.value().name() == "script" {
            reject_terminal_protocol_state(&element.text().collect::<String>(), world)?;
        }
        for (name, value) in element.value().attrs() {
            if name.starts_with("on") {
                reject_terminal_protocol_state(value, world)?;
            } else if matches!(name, "href" | "src" | "action" | "formaction") {
                // HTML 实体已由 DOM 解码；URL 还会移除 tab/CR/LF，不能让拆开的 scheme 绕过检查。
                let normalized = value.replace(['\t', '\r', '\n'], "");
                if let Some((scheme, script)) = normalized
                    .trim_start_matches(|character: char| character <= '\u{20}')
                    .split_once(':')
                    && scheme.eq_ignore_ascii_case("javascript")
                {
                    reject_terminal_protocol_state(script, world)?;
                }
            }
        }
    }

    let fields = parse_detail_fields_for(form, world, operation).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "mooglemail_take",
            "parse_terminal_fields",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;
    let textarea_selector = selector("textarea", operation, world).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_take",
            "build_terminal_textarea_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;
    let textarea = unique_element(form.select(&textarea_selector), operation, world).map_err(
        |diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_take",
                "validate_terminal_textarea",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            action_proof_error(operation, world)
        },
    )?;
    let body = textarea.text().collect::<String>();
    if !valid_message_body(&body) {
        return Err(action_proof_error(operation, world));
    }
    Ok((fields, body))
}

/// 空终态页允许唯一的 reply 按钮：它只打开 compose，不改变邮件状态。
/// 只接受同源、同世界、字段闭集为 `s/ss/reply/filter` 且值为 `Bazaar/mm/<数字>/new` 的
/// `document.location` 赋值；其它脚本动作仍整体失败。
fn is_reply_action(value: &str, world: MoogleMailWorld) -> bool {
    let compact = value.chars().filter(|character| !character.is_whitespace()).collect::<String>();
    let Some(literal) = compact
        .strip_prefix("document.location=")
        .and_then(|rest| rest.strip_prefix('\''))
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        return false;
    };
    let Ok(url) = Url::parse(literal) else { return false };
    if url.scheme() != "https"
        || url.host_str() != Some("hentaiverse.org")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != world.path()
    {
        return false;
    }
    let mut seen = 0_u8;
    for (key, value) in url.query_pairs() {
        let bit = match key.as_ref() {
            "s" if value == "Bazaar" => 1,
            "ss" if value == "mm" => 2,
            "filter" if value == "new" => 4,
            "reply" if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => 8,
            _ => return false,
        };
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
    }
    seen == 0b1111
}

/// 脚本的字符串、注释和不完整赋值中出现协议标记时仍拒绝，避免漏掉属性索引或模板插值。
fn reject_terminal_protocol_state(
    script: &str,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    if ["attach_count", "attach_cod", "mail_state", "equips.set("]
        .iter()
        .any(|marker| script.contains(marker))
    {
        return Err(action_proof_error(MoogleMailOperation::Take, world));
    }
    Ok(())
}

pub(crate) fn prove_take_identity_and_receipts(
    before: &MoogleMailActionState,
    recipient: &str,
    sender: &str,
    subject: &str,
    body: &str,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    let operation = MoogleMailOperation::Take;
    if recipient != before.recipient || sender != before.sender || subject != before.subject {
        return Err(action_proof_error(operation, world));
    }
    let base = canonical_mail_body(&before.body);
    let observed = canonical_mail_body(body);
    let Some(receipts) = observed.strip_prefix(&base) else {
        return Err(action_proof_error(operation, world));
    };
    if (before.cod > 0 && take_cod_receipt_amount(receipts) != Some(before.cod))
        || (before.cod == 0 && receipts.contains("CoD Paid:"))
        || !valid_removed_attachment_receipt(receipts, before.attachment_count)
    {
        return Err(action_proof_error(operation, world));
    }
    Ok(())
}

pub(crate) fn canonical_mail_body(body: &str) -> String {
    let mut canonical = body.replace("\r\n", "\n");
    // 只截短尾部空白，复用换行规范化的原分配，不留下第二份未清理的正文副本。
    canonical.truncate(canonical.trim_end_matches([' ', '\t', '\n']).len());
    canonical
}

/// 计算 Sent/发送证明共用的正文 canonical digest；LF 正文只借用，临时规范文本用后清零。
pub(crate) fn canonical_mail_body_digest(body: &str) -> [u8; 32] {
    if !body.contains("\r\n") {
        return Sha256::digest(body.trim_end_matches([' ', '\t', '\n']).as_bytes()).into();
    }
    let mut canonical = canonical_mail_body(body);
    let digest = Sha256::digest(canonical.as_bytes());
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    let mut bytes = std::mem::take(&mut canonical).into_bytes();
    bytes.fill(0);
    output
}

pub(crate) fn take_cod_receipt_amount(value: &str) -> Option<i64> {
    const MARKER: &str = "CoD Paid: ";
    let mut matches = value.match_indices(MARKER);
    let (start, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let remainder = &value[start + MARKER.len()..];
    let amount_end = remainder
        .find(|character: char| !character.is_ascii_digit() && character != ',')
        .unwrap_or(remainder.len());
    if amount_end == 0 || !remainder[amount_end..].starts_with(" Credits") {
        return None;
    }
    parse_canonical_credits(&remainder[..amount_end])
}

/// 领取后 HV 为每个被取走的附件各追加一行 `Removed attachment: <名称>`。
///
/// 回执行数必须等于领取前的附件数且每行都有内容；没有附件时不允许出现回执行。格式不符
/// 仍失败关闭，转入写入未知恢复路径，绝不自动重发。
pub(crate) fn valid_removed_attachment_receipt(value: &str, expected_count: usize) -> bool {
    const MARKER: &str = "Removed attachment:";
    let mut count = 0_usize;
    for (start, _) in value.match_indices(MARKER) {
        let line = value[start + MARKER.len()..]
            .trim_start_matches([' ', '\t'])
            .split(['\r', '\n'])
            .next()
            .unwrap_or_default();
        if line.trim().is_empty() {
            return false;
        }
        count += 1;
    }
    count == expected_count
}

#[cfg(test)]
mod tests {
    use super::valid_removed_attachment_receipt;

    #[test]
    fn single_attachment_receipt_matches_the_captured_page() {
        // 与 w6_3/taken-3125358.html 相同的回执形状。
        let receipt = "\n\nRemoved attachment: Legendary Radiant Phase Shoes of Niflheim";
        assert!(valid_removed_attachment_receipt(receipt, 1));
        assert!(!valid_removed_attachment_receipt(receipt, 0));
        assert!(!valid_removed_attachment_receipt(receipt, 2));
    }

    #[test]
    fn every_taken_attachment_has_its_own_receipt_line() {
        let receipt = "\n\nRemoved attachment: Legendary Radiant Phase Shoes of Niflheim\n\
                       Removed attachment: Peerless Charged Phase Cap of the Fleet\n\
                       Removed attachment: 49,000 Credits";
        assert!(valid_removed_attachment_receipt(receipt, 3));
        // 行数与领取前附件数不符时不能证明全部取走。
        assert!(!valid_removed_attachment_receipt(receipt, 2));
        assert!(!valid_removed_attachment_receipt(receipt, 4));
    }

    #[test]
    fn empty_or_unexpected_receipt_lines_fail_closed() {
        assert!(valid_removed_attachment_receipt("", 0));
        assert!(!valid_removed_attachment_receipt("", 1));
        assert!(!valid_removed_attachment_receipt("\nRemoved attachment:   \n", 1));
        assert!(!valid_removed_attachment_receipt(
            "\nRemoved attachment: A\nRemoved attachment:\n",
            2
        ));
    }
}

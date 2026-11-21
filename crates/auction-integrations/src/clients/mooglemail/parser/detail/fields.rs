//! 详情页解析的字段、附件与终态证明辅助。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use url::Url;

use crate::clients::mooglemail::parser::{
    MoogleMailActionState, MoogleMailAttachment, action_proof_error, has_result_message,
    parse_canonical_credits, parse_detail_fields_for, parse_token_for_operation, read_selector,
    unique_element, valid_message_body,
};
use crate::clients::mooglemail::{MoogleMailError, MoogleMailOperation, MoogleMailWorld};
use crate::safe_diagnostics;

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

    // 常量选择器不会解析失败；万一失败，`read_selector!` 已记录 build_selector，这里只投影一次。
    let selectors = TerminalSelectors::build(operation, world).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_take",
            "build_terminal_selectors",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        action_proof_error(operation, world)
    })?;
    let envelope = unique_element(document.select(selectors.envelope), operation, world).map_err(
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
    let form = unique_element(document.select(selectors.form), operation, world).map_err(
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
    unique_element(document.select(selectors.panes), operation, world).map_err(
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
    unique_element(document.select(selectors.buttons), operation, world).map_err(
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
    if envelope.select(selectors.all_forms).take(2).count() != 1
        || envelope.select(selectors.form).take(2).count() != 1
        || envelope.select(selectors.panes).take(2).count() != 1
        || envelope.select(selectors.buttons).take(2).count() != 1
    {
        return Err(action_proof_error(operation, world));
    }

    // 导航栏与提示区不属于邮件状态；只在邮件 envelope 内核对剩余动作与资产。
    if envelope.select(selectors.assets).next().is_some()
        || envelope.select(selectors.equipment).next().is_some()
        || envelope.select(selectors.actions).any(|action| {
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
    let textarea = unique_element(form.select(selectors.textarea), operation, world).map_err(
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

/// 空终态页证明所需的常量选择器；集中编译，外层只把失败映射一次为动作证明失败。
struct TerminalSelectors {
    envelope: &'static Selector,
    form: &'static Selector,
    all_forms: &'static Selector,
    panes: &'static Selector,
    buttons: &'static Selector,
    actions: &'static Selector,
    assets: &'static Selector,
    equipment: &'static Selector,
    textarea: &'static Selector,
}

impl TerminalSelectors {
    fn build(
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> Result<Self, MoogleMailError> {
        Ok(Self {
            envelope: read_selector!("#mmail_outer", operation, world)?,
            form: read_selector!("form#mailform", operation, world)?,
            all_forms: read_selector!("form", operation, world)?,
            panes: read_selector!("#mmail_attachpanes", operation, world)?,
            buttons: read_selector!("#mmail_showbuttons", operation, world)?,
            actions: read_selector!("[onclick]", operation, world)?,
            assets: read_selector!(
                "#mmail_attachlist, #mmail_attachcount, #newcod",
                operation,
                world
            )?,
            equipment: read_selector!("[onmouseover]", operation, world)?,
            textarea: read_selector!("textarea", operation, world)?,
        })
    }
}

/// 空终态页允许唯一的 reply 按钮：它只打开 compose，不改变邮件状态。
/// 只接受同源、同世界、字段闭集为 `s/ss/reply/filter` 且值为 `Bazaar/mm/<数字>/new` 的
/// `document.location` 赋值；其它脚本动作仍整体失败。
fn is_reply_action(value: &str, world: MoogleMailWorld) -> bool {
    let compact = value.chars().filter(|character| !character.is_whitespace()).collect::<String>();
    let Some(literal) = compact
        .strip_prefix("document.location=")
        .and_then(|rest| rest.strip_circumfix('\'', '\''))
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

/// 系统退回附加在原正文之后的后缀前缀与结尾（换行已规范为 LF）。
const RETURNED_SUFFIX_PREFIX: &str = "\n\nThis message was returned from ";
const RETURNED_SUFFIX_END: &str = ", kupo!";

/// 结算退件 Take 之后的详情页是否证明附件已被领取：与正常 Take 成功证明同等强度——收件人等于
/// 执行 Take 的凭据所属账号的预期玩家名（正常 Take 要求前后收件人一致；巡检没有领取前页面，以持久
/// 的凭据身份代替，缺失即无法比较、不成立），发件人与主题等于入站快照，正文以原退件正文（原正文
/// 规范摘要 + 系统后缀写明的原收件人）开头，其后的回执恰为领取前附件条目数的
/// `Removed attachment:` 行（领取前带 COD 时另有对应 `CoD Paid:`），且页面不再挂任何资产。退件
/// 正文不入库，只凭证据里的摘要与原收件人重建前缀。
pub(crate) fn prove_returned_take_receipts(
    state: &MoogleMailActionState,
    identity: &auction_application::mail::MoogleMailboxReturnTakeIdentity,
) -> bool {
    if state.recipient.is_empty()
        || identity.expected_recipient.as_deref() != Some(state.recipient.as_str())
        || state.sender != identity.sender
        || state.subject != identity.subject
        || state.claimable
        || !state.equipment_ids.is_empty()
        || state.credits != 0
        || state.cod != 0
        || state.attachment_count != 0
    {
        return false;
    }
    let observed = canonical_mail_body(&state.body);
    let suffix = format!("{RETURNED_SUFFIX_PREFIX}{}{RETURNED_SUFFIX_END}", identity.returned_from);
    // 回执只追加在系统后缀之后；取最后一个后缀，原正文自身含同样文字时摘要比对仍会失败关闭。
    let Some(position) = observed.rfind(&suffix) else {
        return false;
    };
    let original = observed[..position].trim_end_matches([' ', '\t', '\n']);
    let receipts = &observed[position + suffix.len()..];
    !original.is_empty()
        && canonical_mail_body_digest(original) == identity.original_body_digest
        && (if identity.cod > 0 {
            take_cod_receipt_amount(receipts) == Some(identity.cod)
        } else {
            !receipts.contains("CoD Paid:")
        })
        && identity.attachment_count > 0
        && valid_removed_attachment_receipt(receipts, identity.attachment_count)
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
    for line in value.lines() {
        let mut markers = line.match_indices(MARKER);
        let Some((start, _)) = markers.next() else { continue };
        if markers.next().is_some() || !line[..start].trim().is_empty() {
            return false;
        }
        if line[start + MARKER.len()..].trim().is_empty() {
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
        // 与 mooglemail/taken-3125358.html 相同的回执形状。
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

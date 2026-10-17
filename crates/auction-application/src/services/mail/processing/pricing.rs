//! MoogleMail 起拍价指令与装备身份解析。

use super::*;

/// 起拍价解析失败时用于审计的安全诊断；只允许固定代码和数字上下文。
#[derive(Debug, Clone, Copy)]
pub(super) struct PricingFailure {
    pub(super) reason_code: &'static str,
    pub(super) detail_code: &'static str,
    pub(super) body_bytes: usize,
    pub(super) token_count: usize,
    pub(super) field_index: Option<usize>,
    pub(super) attachment_count: usize,
}

struct PricingToken<'a> {
    value: &'a str,
    index: usize,
    is_line_start: bool,
    is_line_end: bool,
}

fn pricing_failure(
    reason_code: &'static str,
    detail_code: &'static str,
    body: &str,
    token_count: usize,
    field_index: Option<usize>,
    attachment_count: usize,
) -> PricingFailure {
    PricingFailure {
        reason_code,
        detail_code,
        body_bytes: body.len(),
        token_count,
        field_index,
        attachment_count,
    }
}

/// 解析拍卖邮件中的兼容起拍价指令。
/// 未填写明确起拍价的附件使用邮件配置默认价；统一 `start amount` 仍可覆盖全部附件，逐件
/// `URL start amount` 指令允许夹在网页说明中并可跨行提取。空行、显式 `#` 注释和普通说明
/// 不计入指令字段，但装备 ID 与 key 仍会和详情页附件交叉校验，避免只凭正文 URL 录入资产。
pub(super) fn resolve_start_price_plan(
    body: &str,
    attachments: &[MailEquipmentAttachment],
    default_price: i64,
) -> Result<Vec<i64>, PricingFailure> {
    const INVALID: &str = "start_price_invalid";
    const MISSING: &str = "start_price_missing";
    const DUPLICATE: &str = "start_price_duplicate";
    const IDENTITY_MISMATCH: &str = "start_price_identity_mismatch";
    const NOT_FOUND: &str = "start_price_attachment_not_found";
    let attachment_count = attachments.len();
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let tokens = collect_pricing_tokens(&normalized);
    // 正文已受 64 KiB 边界限制，诊断记录完整分词数量，而不是首次超限时的数量下界。
    let token_count = tokens.len();

    if default_price <= 0 {
        return Err(pricing_failure(
            INVALID,
            "invalid_default_price",
            body,
            token_count,
            None,
            attachment_count,
        ));
    }
    if attachments.is_empty() {
        return Err(pricing_failure(
            INVALID,
            "invalid_attachment_count",
            body,
            token_count,
            None,
            attachment_count,
        ));
    }
    let mut index_by_id = HashMap::with_capacity(attachments.len());
    for (index, attachment) in attachments.iter().enumerate() {
        if !decimal_identity(&attachment.external_equipment_id)
            || index_by_id.insert(attachment.external_equipment_id.as_str(), index).is_some()
        {
            return Err(pricing_failure(
                INVALID,
                "invalid_attachment_id",
                body,
                token_count,
                None,
                attachment_count,
            ));
        }
    }
    let mut prices = vec![default_price; attachments.len()];
    if !tokens.iter().enumerate().any(|(index, _)| is_explicit_start_token(&tokens, index)) {
        return Ok(prices);
    }
    let maximum_fields = attachments
        .len()
        .checked_mul(3)
        .ok_or_else(|| {
            pricing_failure(
                INVALID,
                "invalid_field_count",
                body,
                token_count,
                None,
                attachment_count,
            )
        })?
        .max(2);
    // 网页复制的 Attachment 说明不能改变指令字段计数；明确的 URL/start 指令允许跨行，
    // 但其余显式 start 形态仍须失败，避免一条合法指令掩盖另一条畸形指令。
    let mut consumed = vec![false; tokens.len()];
    let mut explicit_fields = Vec::new();
    let mut explicit_indices = Vec::new();
    for index in 0..tokens.len() {
        if consumed[index]
            || !tokens[index].value.contains("/equip/")
            || !tokens.get(index + 1).is_some_and(|token| token.value.eq_ignore_ascii_case("start"))
        {
            continue;
        }
        let Some(amount) = tokens.get(index + 2) else {
            return Err(pricing_failure(
                INVALID,
                "invalid_amount",
                body,
                token_count,
                Some(tokens[index + 1].index + 1),
                attachment_count,
            ));
        };
        consumed[index] = true;
        consumed[index + 1] = true;
        consumed[index + 2] = true;
        explicit_fields.extend([tokens[index].value, tokens[index + 1].value, amount.value]);
        explicit_indices.extend([tokens[index].index, tokens[index + 1].index, amount.index]);
    }
    if !explicit_fields.is_empty() {
        for (index, token) in tokens.iter().enumerate() {
            if !consumed[index] && is_explicit_start_token(&tokens, index) {
                let detail_code = if token.value.eq_ignore_ascii_case("start") {
                    "mixed_pricing_directives"
                } else {
                    "invalid_amount"
                };
                return Err(pricing_failure(
                    INVALID,
                    detail_code,
                    body,
                    token_count,
                    Some(token.index),
                    attachment_count,
                ));
            }
        }
    }
    let (fields, field_indices, has_explicit_directive) = if !explicit_fields.is_empty() {
        (explicit_fields, explicit_indices, true)
    } else {
        let mut fields = Vec::with_capacity(maximum_fields);
        let mut field_indices = Vec::with_capacity(maximum_fields);
        for token in &tokens {
            if fields.len() == maximum_fields {
                return Err(pricing_failure(
                    INVALID,
                    "too_many_fields",
                    body,
                    token_count,
                    Some(token.index),
                    attachment_count,
                ));
            }
            fields.push(token.value);
            field_indices.push(token.index);
        }
        (fields, field_indices, false)
    };
    if fields.len() == 2 && fields[0].eq_ignore_ascii_case("start") {
        let price = parse_start_price_amount(fields[1]).ok_or_else(|| {
            pricing_failure(
                INVALID,
                "invalid_amount",
                body,
                token_count,
                Some(field_indices[1]),
                attachment_count,
            )
        })?;
        prices.fill(price);
        return Ok(prices);
    }
    let (directives, remainder) = fields.as_chunks::<3>();
    if !remainder.is_empty() {
        let detail_code =
            if fields.len() == 2 { "expected_start_keyword" } else { "invalid_field_count" };
        return Err(pricing_failure(
            INVALID,
            detail_code,
            body,
            token_count,
            Some(field_indices[fields.len().saturating_sub(remainder.len())]),
            attachment_count,
        ));
    }

    let mut seen = vec![false; attachments.len()];
    for (index, directive) in directives.iter().enumerate() {
        if !directive[1].eq_ignore_ascii_case("start") {
            return Err(pricing_failure(
                INVALID,
                "expected_start_keyword",
                body,
                token_count,
                Some(field_indices[index * 3 + 1]),
                attachment_count,
            ));
        }
        let (equipment_id, equipment_key) = parse_start_equipment_reference(directive[0])
            .ok_or_else(|| {
                pricing_failure(
                    INVALID,
                    "invalid_equipment_reference",
                    body,
                    token_count,
                    Some(field_indices[index * 3]),
                    attachment_count,
                )
            })?;
        let target = *index_by_id.get(equipment_id).ok_or_else(|| {
            pricing_failure(
                NOT_FOUND,
                "attachment_not_found",
                body,
                token_count,
                Some(field_indices[index * 3]),
                attachment_count,
            )
        })?;
        let matched = attachments[target]
            .key
            .with_bytes(|key| key.eq_ignore_ascii_case(equipment_key.as_bytes()))
            .unwrap_or(false);
        if !matched {
            return Err(pricing_failure(
                IDENTITY_MISMATCH,
                "identity_mismatch",
                body,
                token_count,
                Some(field_indices[index * 3]),
                attachment_count,
            ));
        }
        if seen[target] {
            return Err(pricing_failure(
                DUPLICATE,
                "duplicate_attachment",
                body,
                token_count,
                Some(field_indices[index * 3]),
                attachment_count,
            ));
        }
        prices[target] = parse_start_price_amount(directive[2]).ok_or_else(|| {
            pricing_failure(
                INVALID,
                "invalid_amount",
                body,
                token_count,
                Some(field_indices[index * 3 + 2]),
                attachment_count,
            )
        })?;
        seen[target] = true;
    }
    if !has_explicit_directive && seen.iter().any(|found| !found) {
        return Err(pricing_failure(
            MISSING,
            "missing_attachment_directive",
            body,
            token_count,
            None,
            attachment_count,
        ));
    }
    Ok(prices)
}

fn collect_pricing_tokens(body: &str) -> Vec<PricingToken<'_>> {
    body.split('\n')
        .flat_map(|line| {
            let fields = strip_start_price_comment(line).split_whitespace().collect::<Vec<_>>();
            let field_count = fields.len();
            fields.into_iter().enumerate().map(move |(line_index, value)| {
                (line_index == 0, line_index + 1 == field_count, value)
            })
        })
        .enumerate()
        .map(|(index, (is_line_start, is_line_end, value))| PricingToken {
            value,
            index,
            is_line_start,
            is_line_end,
        })
        .collect()
}

/// 识别明确的 start 形态；普通说明中的 starting/starter 或非金额句子不触发定价。
fn is_explicit_start_token(tokens: &[PricingToken<'_>], index: usize) -> bool {
    let value = tokens[index].value;
    let marker = value
        .trim_start_matches(['"', '\'', '`', '[', '('])
        .trim_end_matches(['"', '\'', '`', ']', ')']);
    let Some(suffix) = marker.get(5..).filter(|_| marker[..5].eq_ignore_ascii_case("start")) else {
        return false;
    };
    let compact_amount = suffix
        .trim_start_matches(['+', '-', '.', ','])
        .starts_with(|character: char| character.is_ascii_digit());
    suffix.starts_with(['=', ':'])
        || compact_amount
        || (suffix.is_empty()
            && (index
                .checked_sub(1)
                .and_then(|previous| tokens.get(previous))
                .is_some_and(|token| token.value.contains("/equip/"))
                || tokens.get(index + 1).is_some_and(|token| {
                    token.value.starts_with(|character: char| {
                        character.is_ascii_digit() || matches!(character, '+' | '-' | '.')
                    })
                })
                || (tokens[index].is_line_start
                    && (tokens[index].is_line_end || tokens.len() == 2 && index == 0))))
}

/// 只有行首或空白后的 `#` 才开始注释，装备 URL 自身的 fragment 仍属于 URL token。
fn strip_start_price_comment(value: &str) -> &str {
    value
        .char_indices()
        .find_map(|(index, character)| {
            (character == '#'
                && (index == 0
                    || value[..index].chars().next_back().is_some_and(char::is_whitespace)))
            .then_some(&value[..index])
        })
        .unwrap_or(value)
}

fn parse_start_equipment_reference(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_end_matches(equipment_reference_sentence_punctuation);
    let value = if value.starts_with('<') || value.ends_with('>') {
        value.strip_prefix('<')?.strip_suffix('>')?
    } else {
        value
    };
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let without_fragment = value.split_once('#').map_or(value, |(prefix, _)| prefix);
    let path = without_fragment.split_once('?').map_or(without_fragment, |(prefix, _)| prefix);
    let marker = "/equip/";
    let suffix = path.rsplit_once(marker)?.1;
    let mut fields = suffix.split('/');
    let equipment_id = fields.next()?;
    let equipment_key = fields.next()?;
    if fields.next().is_some()
        || !decimal_identity(equipment_id)
        || equipment_key.len() != 10
        || !equipment_key.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((equipment_id, equipment_key))
}

fn parse_start_price_amount(value: &str) -> Option<i64> {
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    let lowered = value.to_ascii_lowercase();
    let (numeric, factor, decimal_allowed) = if let Some(numeric) = lowered.strip_suffix('k') {
        (numeric, 1_000_i128, true)
    } else if let Some(numeric) = lowered.strip_suffix('m') {
        (numeric, 1_000_000_i128, true)
    } else {
        (lowered.as_str(), 1_i128, false)
    };
    let (integer, fraction) = numeric
        .split_once('.')
        .map_or((numeric, None), |(integer, fraction)| (integer, Some(fraction)));
    let integer = parse_grouped_amount_integer(integer)?;
    let mut amount = integer.checked_mul(factor)?;
    if let Some(fraction) = fraction {
        if !decimal_allowed
            || fraction.is_empty()
            || fraction.contains('.')
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let scale = 10_i128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
        let scaled_fraction = parse_ascii_amount_digits(fraction)?.checked_mul(factor)?;
        if scaled_fraction % scale != 0 {
            return None;
        }
        amount = amount.checked_add(scaled_fraction / scale)?;
    }
    i64::try_from(amount).ok().filter(|amount| *amount > 0)
}

fn equipment_reference_sentence_punctuation(character: char) -> bool {
    matches!(character, '.' | ',' | ';' | '!' | '?' | '。' | '，' | '；' | '！' | '？')
}

fn parse_grouped_amount_integer(value: &str) -> Option<i128> {
    if !value.contains(',') {
        return parse_ascii_amount_digits(value);
    }
    let mut groups = value.split(',');
    let first = groups.next()?;
    if first.is_empty() || first.len() > 3 {
        return None;
    }
    let mut amount = parse_ascii_amount_digits(first)?;
    for group in groups {
        if group.len() != 3 {
            return None;
        }
        amount = amount.checked_mul(1_000)?.checked_add(parse_ascii_amount_digits(group)?)?;
    }
    Some(amount)
}

fn parse_ascii_amount_digits(value: &str) -> Option<i128> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.bytes().try_fold(0_i128, |amount, digit| {
        amount.checked_mul(10)?.checked_add(i128::from(digit - b'0'))
    })
}

pub(super) fn decimal_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

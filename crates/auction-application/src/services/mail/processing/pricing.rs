//! MoogleMail 起拍价指令与装备身份解析。

use super::*;

/// 解析拍卖邮件中的兼容起拍价指令。
///
/// 空白正文使用配置默认价；单条 `start amount` 对全部附件生效，其余输入按连续的
/// `URL start amount` 三元组解析。空行、常见换行、显式 `#` 注释和物理行边界不影响指令，
/// 但装备 ID 与 key 仍会同时和详情页附件交叉校验，避免只凭正文中的可伪造 URL 录入资产。
pub(super) fn resolve_start_price_plan(
    body: &str,
    attachments: &[MailEquipmentAttachment],
    default_price: i64,
) -> Result<Vec<i64>, &'static str> {
    const INVALID: &str = "start_price_invalid";
    const MISSING: &str = "start_price_missing";
    const DUPLICATE: &str = "start_price_duplicate";
    const IDENTITY_MISMATCH: &str = "start_price_identity_mismatch";
    const NOT_FOUND: &str = "start_price_attachment_not_found";

    if default_price <= 0 || attachments.is_empty() {
        return Err(INVALID);
    }
    let mut index_by_id = HashMap::with_capacity(attachments.len());
    for (index, attachment) in attachments.iter().enumerate() {
        if !decimal_identity(&attachment.external_equipment_id)
            || index_by_id.insert(attachment.external_equipment_id.as_str(), index).is_some()
        {
            return Err(INVALID);
        }
    }
    let mut prices = vec![default_price; attachments.len()];
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let maximum_fields = attachments.len().checked_mul(3).ok_or(INVALID)?.max(2);
    let mut fields = Vec::with_capacity(maximum_fields);
    for raw_line in normalized.split('\n') {
        for field in strip_start_price_comment(raw_line).split_whitespace() {
            if fields.len() == maximum_fields {
                return Err(INVALID);
            }
            fields.push(field);
        }
    }
    if fields.is_empty() {
        return Ok(prices);
    }
    if fields.len() == 2 && fields[0].eq_ignore_ascii_case("start") {
        let price = parse_start_price_amount(fields[1]).ok_or(INVALID)?;
        prices.fill(price);
        return Ok(prices);
    }
    let (directives, remainder) = fields.as_chunks::<3>();
    if !remainder.is_empty() {
        return Err(INVALID);
    }

    let mut seen = vec![false; attachments.len()];
    for directive in directives {
        if !directive[1].eq_ignore_ascii_case("start") {
            return Err(INVALID);
        }
        let (equipment_id, equipment_key) =
            parse_start_equipment_reference(directive[0]).ok_or(INVALID)?;
        let target = *index_by_id.get(equipment_id).ok_or(NOT_FOUND)?;
        let matched = attachments[target]
            .key
            .with_bytes(|key| key.eq_ignore_ascii_case(equipment_key.as_bytes()))
            .unwrap_or(false);
        if !matched {
            return Err(IDENTITY_MISMATCH);
        }
        if seen[target] {
            return Err(DUPLICATE);
        }
        prices[target] = parse_start_price_amount(directive[2]).ok_or(INVALID)?;
        seen[target] = true;
    }
    if seen.iter().any(|found| !found) {
        return Err(MISSING);
    }
    Ok(prices)
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

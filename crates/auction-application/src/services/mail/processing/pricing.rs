//! MoogleMail 起拍价指令与装备身份解析。

use super::*;

/// 严格解析旧版拍卖邮件中的起拍价指令。
///
/// 空正文使用配置默认价；非空正文必须为每个附件提供恰好一条 `URL start amount` 指令。
/// 装备 ID 与 key 会同时和详情页附件交叉校验，避免只凭正文中的可伪造 URL 录入资产。
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
    if body.is_empty() {
        return Ok(prices);
    }
    let normalized = body.replace("\r\n", "\n");
    if normalized.contains('\r') {
        return Err(INVALID);
    }
    let mut seen = vec![false; attachments.len()];
    for raw_line in normalized.split('\n') {
        let fields = raw_line.split_whitespace().collect::<Vec<_>>();
        let (target, amount_field) = if attachments.len() == 1
            && fields.len() == 2
            && fields[0].eq_ignore_ascii_case("start")
        {
            (0, fields[1])
        } else if fields.len() == 3 && fields[1].eq_ignore_ascii_case("start") {
            let (equipment_id, equipment_key) =
                parse_start_equipment_reference(fields[0]).ok_or(INVALID)?;
            let target = *index_by_id.get(equipment_id).ok_or(NOT_FOUND)?;
            let matched = attachments[target]
                .key
                .with_bytes(|key| key.eq_ignore_ascii_case(equipment_key.as_bytes()))
                .unwrap_or(false);
            if !matched {
                return Err(IDENTITY_MISMATCH);
            }
            (target, fields[2])
        } else {
            return Err(INVALID);
        };
        if seen[target] {
            return Err(DUPLICATE);
        }
        prices[target] = parse_start_price_amount(amount_field).ok_or(INVALID)?;
        seen[target] = true;
    }
    if seen.iter().any(|found| !found) {
        return Err(MISSING);
    }
    Ok(prices)
}

fn parse_start_equipment_reference(value: &str) -> Option<(&str, &str)> {
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
    let (digits, factor) = if let Some(digits) = lowered.strip_suffix('k') {
        (digits, 1_000_i64)
    } else if let Some(digits) = lowered.strip_suffix('m') {
        (digits, 1_000_000_i64)
    } else {
        (lowered.as_str(), 1_i64)
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok()?.checked_mul(factor).filter(|amount| *amount > 0)
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

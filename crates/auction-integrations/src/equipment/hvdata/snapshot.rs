//! 固定装备来源的字段校验及白名单投影。

use auction_application::consignment::{
    ListingEquipmentDetails, ListingEquipmentLabel, ListingEquipmentLevel,
    ListingEquipmentLookupError, ListingEquipmentStat, ListingEquipmentStatGroup,
    ListingLookupSnapshot, LookupPermanentReason,
};
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::{EquipmentKey, valid_positive_decimal, valid_text};
use auction_time::Timestamp;
use subtle::ConstantTimeEq;

use super::json::{Object, Value, finite_number};

type Reason = LookupPermanentReason;
type LookupError = ListingEquipmentLookupError;

pub(super) fn snapshot(
    object: &Object<'_>,
    identity: &ListingEquipmentIdentity,
    key: &EquipmentKey,
    observed_at: Timestamp,
) -> Result<ListingLookupSnapshot, LookupError> {
    validate_identity(object, identity, key)?;
    let owner_name = owner_name(object)?;
    let (level, tradeable) = confirmed_level(object)?;
    let label = ListingEquipmentLabel {
        name: field_text(object, "name")?.to_owned(),
        alt_name: object.get("alt_name").map(require_text).transpose()?.map(str::to_owned),
        category: field_text(object, "category")?.to_owned(),
        level,
        tradeable,
    };
    validate_name_parts(object)?;
    let details = ListingEquipmentDetails::new(observed_at, label, stat_groups(object)?)?;
    ListingLookupSnapshot::new(identity.clone(), details, owner_name.to_owned(), key)
}

fn validate_identity(
    object: &Object<'_>,
    identity: &ListingEquipmentIdentity,
    key: &EquipmentKey,
) -> Result<(), LookupError> {
    // 不变量 6、7：以十进制原 token 核对身份，并以等长常量时间比较规范化后的凭证。
    let eid = object.get("eid").and_then(Value::number).ok_or(Reason::SourceSchemaInvalid)?;
    if !valid_positive_decimal(eid, 32) {
        return Err(Reason::SourceSchemaInvalid.into());
    }
    if eid != identity.external_equipment_id() {
        return Err(Reason::SourceIdentityMismatch.into());
    }
    let Ok(echoed) = EquipmentKey::new(field_text(object, "key")?) else {
        return Err(Reason::SourceSchemaInvalid.into());
    };
    let Ok(matches_key) =
        key.with_bytes(|expected| echoed.with_bytes(|actual| bool::from(expected.ct_eq(actual))))
    else {
        return Err(LookupError::CredentialUnavailable);
    };
    let Ok(matches_key) = matches_key else {
        return Err(Reason::SourceSchemaInvalid.into());
    };
    if !matches_key {
        return Err(Reason::SourceIdentityMismatch.into());
    }
    match object.get("is_isekai") {
        None => (),
        Some(Value::Bool(value)) if *value == (identity.world_id() == "isekai") => (),
        Some(Value::Bool(_)) => return Err(Reason::SourceIdentityMismatch.into()),
        Some(_) => return Err(Reason::SourceSchemaInvalid.into()),
    }
    if let Some(world) = object.get("world_id")
        && require_text(world)? != identity.world_id()
    {
        return Err(Reason::SourceIdentityMismatch.into());
    }
    Ok(())
}

fn owner_name<'a>(object: &'a Object<'_>) -> Result<&'a str, Reason> {
    let owner = match object.get("owner") {
        None | Some(Value::Null) => return Err(Reason::OwnerMissing),
        Some(value) => value.object().ok_or(Reason::SourceSchemaInvalid)?,
    };
    let name = require_text(owner.get("name").ok_or(Reason::OwnerMissing)?)?;
    if let Some(uid) = owner.get("uid") {
        let token = uid.number().ok_or(Reason::SourceSchemaInvalid)?;
        if !valid_positive_decimal(token, token.len()) {
            return Err(Reason::SourceSchemaInvalid);
        }
    }
    Ok(name)
}

fn confirmed_level(object: &Object<'_>) -> Result<(ListingEquipmentLevel, bool), Reason> {
    let Some(Value::Bool(tradeable)) = object.get("is_tradeable") else {
        return Err(Reason::SourceSchemaInvalid);
    };
    // 拿到可交易样本后在此开放对应分支；完整魂绑观察交给用例判定不可挂单。
    match (object.get("level"), tradeable) {
        (Some(Value::Text(level)), false) if level == "Soulbound" => {
            Ok((ListingEquipmentLevel::Soulbound, false))
        }
        (Some(Value::Text(level)), true) if level == "Soulbound" => {
            Err(Reason::SourceSchemaInvalid)
        }
        (Some(_), _) => Err(Reason::SourceShapeUnconfirmed),
        (None, _) => Err(Reason::SourceSchemaInvalid),
    }
}

fn validate_name_parts(object: &Object<'_>) -> Result<(), Reason> {
    let Some(parts) = object.get("name_parts") else {
        return Ok(());
    };
    let parts = parts.object().ok_or(Reason::SourceSchemaInvalid)?;
    for name in
        ["tier", "prefix", "type", "armor_type", "slot_type", "weapon_type", "suf_art", "suffix"]
    {
        let allowed = match parts.get(name) {
            None => true,
            Some(Value::Null) => matches!(name, "armor_type" | "slot_type"),
            Some(Value::Text(value)) => {
                valid_text(value, usize::MAX) && !value.contains(['\u{2028}', '\u{2029}'])
            }
            Some(_) => false,
        };
        if !allowed {
            return Err(Reason::SourceSchemaInvalid);
        }
    }
    // 当前应用快照没有 name_parts；拒绝未证实形状，只使用独立给出的 name。
    Ok(())
}

fn stat_groups(object: &Object<'_>) -> Result<Vec<ListingEquipmentStatGroup>, Reason> {
    let mut count = 0;
    object_field(object, "stats")?
        .iter()
        .map(|(name, group)| {
            let group = group.object().ok_or(Reason::SourceSchemaInvalid)?;
            count += group.len();
            if count > 256 {
                return Err(Reason::SourceTooLarge);
            }
            let stats = group
                .iter()
                .map(|(name, stat)| {
                    let stat = stat.object().ok_or(Reason::SourceSchemaInvalid)?;
                    let value = stat.get("value").ok_or(Reason::SourceSchemaInvalid)?;
                    Ok(ListingEquipmentStat {
                        name: name.clone(),
                        value: stat_value(value)?,
                        base: stat.get("base").map(stat_value).transpose()?,
                    })
                })
                .collect::<Result<_, Reason>>()?;
            Ok(ListingEquipmentStatGroup { name: name.clone(), stats })
        })
        .collect()
}

fn stat_value(value: &Value<'_>) -> Result<f64, Reason> {
    let token = value.number().ok_or(Reason::SourceSchemaInvalid)?;
    let number = finite_number(token)?;
    let magnitude = number.abs();
    // 二进制浮点可能把刚越界的原数舍入到上限；在相等时核对十进制 token。
    if magnitude > 1e9 || (magnitude == 1e9 && !within_stat_boundary(token)) {
        return Err(Reason::SourceSchemaInvalid);
    }
    Ok(number)
}

fn within_stat_boundary(token: &str) -> bool {
    let token = token.trim_start_matches('-');
    let (significand, exponent) = token.split_once(['e', 'E']).unwrap_or((token, "0"));
    let Ok(exponent) = exponent.parse::<i64>() else {
        return false;
    };
    let integer_digits = significand.split('.').next().unwrap_or_default().len() as i64;
    let mut digits = significand.bytes().filter(u8::is_ascii_digit).peekable();
    let mut leading_zeroes = 0;
    while digits.peek() == Some(&b'0') {
        digits.next();
        leading_zeroes += 1;
    }
    let first = digits.next();
    match integer_digits.saturating_add(exponent).saturating_sub(leading_zeroes) {
        10 => first == Some(b'1') && digits.all(|digit| digit == b'0'),
        digits_before_point => digits_before_point < 10,
    }
}

fn object_field<'a, 'body>(
    object: &'a Object<'body>,
    name: &str,
) -> Result<&'a Object<'body>, Reason> {
    match object.get(name) {
        Some(Value::Object(value)) => Ok(value),
        _ => Err(Reason::SourceSchemaInvalid),
    }
}

fn field_text<'a>(object: &'a Object<'_>, name: &str) -> Result<&'a str, Reason> {
    require_text(object.get(name).ok_or(Reason::SourceSchemaInvalid)?)
}

fn require_text<'a>(value: &'a Value<'_>) -> Result<&'a str, Reason> {
    value.text().ok_or(Reason::SourceSchemaInvalid)
}

pub(super) fn known_not_found(object: &Object<'_>) -> bool {
    object.len() == 1 && object.get("detail").and_then(Value::text) == Some("Not Found")
}

pub(super) fn known_rejected_request(object: &Object<'_>) -> bool {
    // 只识别样本证实的缺 query 参数；其他错误形态失败关闭，不取出任何 input 内容。
    let Some(Value::Array(details)) = object.get("detail") else {
        return false;
    };
    object.len() == 1
        && !details.is_empty()
        && details.iter().all(|detail| {
            let Some(detail) = detail.object() else {
                return false;
            };
            let Some(Value::Array(location)) = detail.get("loc") else {
                return false;
            };
            detail.len() == 4
                && detail.get("type").and_then(Value::text) == Some("missing")
                && detail.get("msg").and_then(Value::text) == Some("Field required")
                && matches!(detail.get("input"), Some(Value::Null))
                && matches!(location.as_slice(), [scope, name]
                if scope.text() == Some("query")
                && matches!(name.text(), Some("eid" | "key" | "is_isekai")))
        })
}

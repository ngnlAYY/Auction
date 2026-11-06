//! 装备来源 URL、主机和路径约束。

use auction_application::admin::AdminIntakeError;
use auction_domain::EquipmentKey;
use url::Url;

use super::super::config::EquipmentSourcePolicy;

pub(crate) fn parse_equipment_url(
    endpoint: &Url,
    policy: &EquipmentSourcePolicy,
    world_id: &str,
    raw_url: &str,
) -> Result<(Url, String, String), AdminIntakeError> {
    if !matches!(world_id, "main" | "isekai")
        || !auction_domain::valid_text(raw_url, 2048)
        || raw_url.contains('\\')
    {
        return Err(AdminIntakeError::InvalidCommand);
    }
    let normalized_url = normalize_equipment_url(raw_url).ok_or(AdminIntakeError::InvalidSource)?;
    let parsed = Url::parse(normalized_url).map_err(|_| AdminIntakeError::InvalidSource)?;
    if parsed.scheme() != endpoint.scheme()
        || parsed.host_str() != endpoint.host_str()
        || parsed.port_or_known_default() != endpoint.port_or_known_default()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path().contains("..")
        || parsed.as_str() != normalized_url
        || !policy.allows(&parsed)
    {
        return Err(AdminIntakeError::InvalidSource);
    }
    let prefix = if world_id == "isekai" { "/isekai/equip/" } else { "/equip/" };
    let path = parsed.path().strip_prefix(prefix).ok_or(AdminIntakeError::InvalidSource)?;
    let (equipment_id, key) = path.split_once('/').ok_or(AdminIntakeError::InvalidSource)?;
    // key 的领域校验也排除额外路径段；借用路径片段，避免建立集合和重复复制 ID。
    if !valid_decimal_id(equipment_id) || EquipmentKey::new(key).is_err() {
        return Err(AdminIntakeError::InvalidSource);
    }
    let equipment_id = equipment_id.to_owned();
    let key = key.to_ascii_lowercase();
    Ok((parsed, equipment_id, key))
}

/// 兼容从自然语言复制的 URL 包裹和句末标点；规范化后仍执行完整 origin、路径和 key 校验。
fn normalize_equipment_url(value: &str) -> Option<&str> {
    let value = value.trim_end_matches(equipment_url_sentence_punctuation);
    let value = if value.starts_with('<') || value.ends_with('>') {
        value.strip_prefix('<')?.strip_suffix('>')?
    } else {
        value
    };
    (!value.is_empty()).then_some(value)
}

fn equipment_url_sentence_punctuation(character: char) -> bool {
    matches!(character, '.' | ',' | ';' | '!' | '?' | '。' | '，' | '；' | '！' | '？')
}

fn valid_decimal_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

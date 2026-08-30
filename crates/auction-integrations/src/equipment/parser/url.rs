//! 装备来源 URL、主机和路径约束。

use auction_application::AdminIntakeError;
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
        || raw_url.is_empty()
        || raw_url != raw_url.trim()
        || raw_url.len() > 2048
        || raw_url.contains('\\')
        || raw_url.chars().any(char::is_control)
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
    let parts: Vec<String> = parsed
        .path()
        .strip_prefix(prefix)
        .unwrap_or_default()
        .split('/')
        .map(str::to_owned)
        .collect();
    if parts.len() != 2 || !valid_decimal_id(&parts[0]) || EquipmentKey::new(&parts[1]).is_err() {
        return Err(AdminIntakeError::InvalidSource);
    }
    Ok((parsed, parts[0].clone(), parts[1].to_ascii_lowercase()))
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

pub(crate) fn valid_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value == value.to_ascii_lowercase()
        && value.parse::<std::net::IpAddr>().is_err()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

pub(crate) fn valid_path_prefix(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with('/')
        && value.ends_with('/')
        && value == value.trim()
        && !value.contains("//")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '?' | '#' | '%' | '\\'))
        && value.split('/').all(|segment| segment != "." && segment != "..")
}

fn valid_decimal_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

//! 旧版装备页、URL 与统计字段解析。
//!
//! 解析器只接受固定的结构、闭集类别和有界文本；任何无法证明的 HTML 或字段都会转换为
//! `InvalidSource`，原始页面不会继续流入应用层。

use std::collections::BTreeMap;

use auction_application::AdminIntakeError;
use auction_domain::EquipmentKey;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use super::{
    config::EquipmentSourcePolicy,
    model::{EquipmentFacts, EquipmentStat},
};

pub(super) fn parse_equipment_url(
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
    let parsed = Url::parse(raw_url).map_err(|_| AdminIntakeError::InvalidSource)?;
    if parsed.scheme() != endpoint.scheme()
        || parsed.host_str() != endpoint.host_str()
        || parsed.port_or_known_default() != endpoint.port_or_known_default()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path().contains("..")
        || parsed.as_str() != raw_url
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

pub(super) fn parse_equipment_page(
    body: &[u8],
    external_equipment_id: &str,
) -> Result<EquipmentFacts, AdminIntakeError> {
    let document = std::str::from_utf8(body).map_err(|_| AdminIntakeError::InvalidSource)?;
    let document = Html::parse_document(document);
    let panel_selector = selector("div.showequip")?;
    let panels: Vec<ElementRef<'_>> = document.select(&panel_selector).collect();
    if panels.len() != 1 {
        return Err(AdminIntakeError::InvalidSource);
    }
    let panel = panels[0];
    let child_selector = selector("div.showequip > div")?;
    let children: Vec<ElementRef<'_>> = panel.select(&child_selector).collect();
    if children.len() < 2 {
        return Err(AdminIntakeError::InvalidSource);
    }
    let name = normalize_text(children[0].text().collect::<String>());
    if !valid_text(&name, 512) {
        return Err(AdminIntakeError::InvalidSource);
    }
    let header_selector = selector(".eqt")?;
    let header = children[1]
        .select(&header_selector)
        .next()
        .map(|value| normalize_text(value.text().collect::<String>()))
        .ok_or(AdminIntakeError::InvalidSource)?;
    let (category, level, tradeable) = parse_header(&header)?;
    let (quality, prefix, equipment_type, suffix) = parse_identity(&name)?;
    if category_for_type(&equipment_type).as_deref() != Some(category.as_str()) || !tradeable {
        return Err(AdminIntakeError::InvalidSource);
    }
    let stats = parse_stats(children[1])?;
    Ok(EquipmentFacts {
        external_equipment_id: external_equipment_id.to_owned(),
        name,
        category,
        equipment_type,
        quality,
        prefix,
        suffix,
        level,
        tradeable,
        stats,
    })
}

fn parse_header(value: &str) -> Result<(String, Option<i32>, bool), AdminIntakeError> {
    let mut fields: Vec<&str> = value.split_whitespace().collect();
    let tradeable = fields.last().is_some_and(|field| *field == "Tradeable");
    if fields.last().is_some_and(|field| *field == "Tradeable" || *field == "Soulbound") {
        fields.pop();
    }
    let mut level = None;
    if let Some(index) = fields.iter().position(|field| *field == "Level") {
        let raw = fields.get(index + 1).ok_or(AdminIntakeError::InvalidSource)?;
        let parsed = raw.parse::<i32>().map_err(|_| AdminIntakeError::InvalidSource)?;
        if !(1..=100_000).contains(&parsed) {
            return Err(AdminIntakeError::InvalidSource);
        }
        level = Some(parsed);
        fields.drain(index..=index + 1);
    }
    let category = normalize_category(&fields.join(" ")).ok_or(AdminIntakeError::InvalidSource)?;
    Ok((category, level, tradeable))
}

pub(super) fn parse_identity(
    name: &str,
) -> Result<(String, String, String, String), AdminIntakeError> {
    let fields: Vec<&str> = name.split_whitespace().collect();
    if fields.len() < 2 {
        return Err(AdminIntakeError::InvalidSource);
    }
    let qualities = [
        "Peerless",
        "Legendary",
        "Magnificent",
        "Exquisite",
        "Superior",
        "Average",
        "Fair",
        "Crude",
    ];
    let quality = qualities
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(fields[0]))
        .ok_or(AdminIntakeError::InvalidSource)?
        .to_string();
    let body = &fields[1..];
    let separator = body.iter().position(|field| field.eq_ignore_ascii_case("of"));
    let (type_fields, suffix) = match separator {
        Some(index) if index > 0 && index + 1 < body.len() => {
            (&body[..index], body[index + 1..].join(" "))
        }
        Some(_) => return Err(AdminIntakeError::InvalidSource),
        None => (body, String::new()),
    };
    let joined = type_fields.join(" ");
    let types = equipment_types();
    let mut candidates: Vec<&str> = types.iter().map(|(_, value)| *value).collect();
    candidates.sort_by_key(|value| std::cmp::Reverse(value.split_whitespace().count()));
    let mut selected = None;
    for candidate in candidates {
        if joined.eq_ignore_ascii_case(candidate) {
            selected = Some((candidate.to_owned(), String::new()));
            break;
        }
        if joined.len() > candidate.len()
            && joined
                .get(joined.len() - candidate.len()..)
                .is_some_and(|value| value.eq_ignore_ascii_case(candidate))
            && joined.as_bytes().get(joined.len() - candidate.len() - 1) == Some(&b' ')
        {
            let prefix = joined[..joined.len() - candidate.len() - 1].trim().to_owned();
            selected = Some((candidate.to_owned(), prefix));
            break;
        }
    }
    let (equipment_type, prefix) = selected.ok_or(AdminIntakeError::InvalidSource)?;
    if !valid_optional_text(&prefix, 128) || !valid_optional_text(&suffix, 128) {
        return Err(AdminIntakeError::InvalidSource);
    }
    Ok((quality, prefix, equipment_type, suffix))
}

fn parse_stats(panel: ElementRef<'_>) -> Result<BTreeMap<String, EquipmentStat>, AdminIntakeError> {
    let mut stats = BTreeMap::new();
    let title_selector = selector("[title]")?;
    for element in panel.select(&title_selector) {
        let Some(title) = element.value().attr("title") else { continue };
        let Some(base) = parse_base(title) else { continue };
        let text = normalize_text(element.text().collect::<String>());
        let (name, value) = parse_main_stat(&text).ok_or(AdminIntakeError::InvalidSource)?;
        insert_stat(&mut stats, name, value, Some(base))?;
    }
    let row_selector = selector(".ex > div, .ep > div")?;
    for row in panel.select(&row_selector) {
        let elements: Vec<ElementRef<'_>> = row.children().filter_map(ElementRef::wrap).collect();
        if elements.len() < 2 || elements.len() > 3 {
            continue;
        }
        let name = normalize_text(elements[0].text().collect::<String>());
        let value = normalize_stat_value(&normalize_text(elements[1].text().collect::<String>()))
            .ok_or(AdminIntakeError::InvalidSource)?;
        if elements.len() == 3 {
            let unit = normalize_text(elements[2].text().collect::<String>());
            if unit != "%" && unit != "x" {
                return Err(AdminIntakeError::InvalidSource);
            }
        }
        let base = row.value().attr("title").and_then(parse_base);
        insert_stat(&mut stats, name, value, base)?;
    }
    Ok(stats)
}

fn insert_stat(
    stats: &mut BTreeMap<String, EquipmentStat>,
    name: String,
    value: String,
    base: Option<f64>,
) -> Result<(), AdminIntakeError> {
    if !valid_text(&name, 128) || stats.insert(name, EquipmentStat { value, base }).is_some() {
        return Err(AdminIntakeError::InvalidSource);
    }
    if stats.len() > 256 {
        return Err(AdminIntakeError::InvalidSource);
    }
    Ok(())
}

fn parse_main_stat(value: &str) -> Option<(String, String)> {
    let mut fields = value.splitn(2, char::is_whitespace);
    let raw_value = fields.next()?;
    let name = normalize_text(fields.next()?.trim());
    let normalized = normalize_stat_value(raw_value)?;
    if !normalized.starts_with('+') || !valid_text(&name, 128) {
        return None;
    }
    Some((name, normalized))
}

fn parse_base(value: &str) -> Option<f64> {
    let value = value.trim().strip_prefix("Base:")?.trim();
    let parsed = value.parse::<f64>().ok()?;
    parsed.is_finite().then_some(parsed).filter(|value| (0.0..=1_000_000.0).contains(value))
}

pub(super) fn normalize_stat_value(value: &str) -> Option<String> {
    let value = value.trim();
    let first = value.as_bytes().first().copied()?;
    if !matches!(first, b'+' | b'-') || value.len() == 1 {
        return None;
    }
    let mut dot_seen = false;
    let mut digits = 0;
    let mut normalized = String::with_capacity(value.len());
    normalized.push(first as char);
    for byte in value.as_bytes()[1..].iter().copied() {
        match byte {
            b'0'..=b'9' => {
                digits += 1;
                normalized.push(byte as char);
            }
            b',' if !dot_seen => {}
            b'.' if !dot_seen => {
                dot_seen = true;
                normalized.push('.');
            }
            _ => return None,
        }
    }
    (digits > 0).then_some(normalized)
}

fn normalize_category(value: &str) -> Option<String> {
    match value
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .as_str()
    {
        "one handed weapon" => Some("One-Handed".to_owned()),
        "two handed weapon" => Some("Two-Handed".to_owned()),
        "staff" => Some("Staff".to_owned()),
        "shield" => Some("Shield".to_owned()),
        "cloth armor" => Some("Cloth".to_owned()),
        "light armor" => Some("Light".to_owned()),
        "heavy armor" => Some("Heavy".to_owned()),
        _ => None,
    }
}

pub(super) fn category_for_type(value: &str) -> Option<String> {
    equipment_types()
        .into_iter()
        .find(|(_, candidate)| *candidate == value)
        .map(|(category, _)| category.to_owned())
}

fn equipment_types() -> Vec<(&'static str, &'static str)> {
    vec![
        ("One-Handed", "Axe"),
        ("One-Handed", "Club"),
        ("One-Handed", "Rapier"),
        ("One-Handed", "Shortsword"),
        ("One-Handed", "Wakizashi"),
        ("Two-Handed", "Estoc"),
        ("Two-Handed", "Longsword"),
        ("Two-Handed", "Mace"),
        ("Two-Handed", "Katana"),
        ("Staff", "Oak Staff"),
        ("Staff", "Willow Staff"),
        ("Staff", "Redwood Staff"),
        ("Staff", "Katalox Staff"),
        ("Shield", "Buckler"),
        ("Shield", "Kite Shield"),
        ("Shield", "Force Shield"),
        ("Cloth", "Cotton Cap"),
        ("Cloth", "Phase Cap"),
        ("Cloth", "Cotton Robe"),
        ("Cloth", "Phase Robe"),
        ("Cloth", "Cotton Gloves"),
        ("Cloth", "Phase Gloves"),
        ("Cloth", "Cotton Pants"),
        ("Cloth", "Phase Pants"),
        ("Cloth", "Cotton Shoes"),
        ("Cloth", "Phase Shoes"),
        ("Light", "Leather Cap"),
        ("Light", "Shade Helmet"),
        ("Light", "Leather Breastplate"),
        ("Light", "Shade Breastplate"),
        ("Light", "Leather Gauntlets"),
        ("Light", "Shade Gauntlets"),
        ("Light", "Leather Leggings"),
        ("Light", "Shade Leggings"),
        ("Light", "Leather Boots"),
        ("Light", "Shade Boots"),
        ("Heavy", "Plate Helmet"),
        ("Heavy", "Power Helmet"),
        ("Heavy", "Plate Cuirass"),
        ("Heavy", "Power Armor"),
        ("Heavy", "Plate Gauntlets"),
        ("Heavy", "Power Gauntlets"),
        ("Heavy", "Plate Greaves"),
        ("Heavy", "Power Leggings"),
        ("Heavy", "Plate Sabatons"),
        ("Heavy", "Power Boots"),
    ]
}

fn selector(raw: &str) -> Result<Selector, AdminIntakeError> {
    Selector::parse(raw).map_err(|_| AdminIntakeError::InvalidSource)
}

fn normalize_text(value: impl AsRef<str>) -> String {
    value.as_ref().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn valid_dns_name(value: &str) -> bool {
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

pub(super) fn valid_path_prefix(value: &str) -> bool {
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

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

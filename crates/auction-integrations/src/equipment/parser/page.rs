//! 旧版装备 HTML 页面与统计字段解析。

use std::collections::BTreeMap;

use auction_application::AdminIntakeError;
use scraper::{ElementRef, Html};

use super::super::model::{EquipmentFacts, EquipmentStat};
use super::{
    common::{normalize_text, selector, valid_text},
    identity::{category_for_type, parse_identity},
    stats::normalize_stat_value,
};

pub(crate) fn parse_equipment_page(
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

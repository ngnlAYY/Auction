//! 装备页面的 HTML 结构和属性提取；不推断名称语义、装备大类或交易资格。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{collections::BTreeMap, sync::LazyLock};

use scraper::{ElementRef, Html, Selector};

use super::super::model::EquipmentStat;
use super::{
    EquipmentParseError,
    common::{normalize_text, valid_text},
    stats::normalize_stat_value,
};

struct EquipmentSelectors {
    panel: Selector,
    children: Selector,
    header: Selector,
    equipment: Selector,
    main: Selector,
    groups: Selector,
}

// 选择器来自固定源码；每进程只编译一次，错误仍按页面结构错误返回，不缓存外部正文。
static SELECTORS: LazyLock<Result<EquipmentSelectors, ()>> = LazyLock::new(|| {
    let parse = |value| {
        Selector::parse(value).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "equipment_fetch",
                "build_equipment_selectors",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
        })
    };
    Ok(EquipmentSelectors {
        panel: parse("div.showequip")?,
        children: parse("div.showequip > div")?,
        header: parse(".eqt")?,
        equipment: parse(".eq")?,
        main: parse(".eq > div")?,
        groups: parse(".ex, .ep")?,
    })
});

/// HTML 提供的原始字段，身份语义与 header 状态由独立模块校验。
pub(super) struct EquipmentPageFields {
    pub(super) name: String,
    pub(super) header: String,
    pub(super) stats: BTreeMap<String, EquipmentStat>,
}

pub(super) fn extract_equipment_page(
    body: &[u8],
) -> Result<EquipmentPageFields, EquipmentParseError> {
    let document = std::str::from_utf8(body).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "equipment_fetch",
            "decode_equipment_page",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        EquipmentParseError::PageStructure
    })?;
    let document = Html::parse_document(document);
    let selectors = SELECTORS.as_ref().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "equipment_fetch",
            "read_equipment_selectors",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        EquipmentParseError::PageStructure
    })?;
    let mut panels = document.select(&selectors.panel);
    let panel = panels.next().ok_or(EquipmentParseError::PageStructure)?;
    if panels.next().is_some() {
        return Err(EquipmentParseError::PageStructure);
    }
    let mut children = panel.select(&selectors.children);
    let name_element = children.next().ok_or(EquipmentParseError::PageStructure)?;
    let details = children.next().ok_or(EquipmentParseError::PageStructure)?;
    let name = normalize_text(name_element.text().collect::<String>());
    if !valid_text(&name, 512) {
        return Err(EquipmentParseError::InvalidName);
    }
    let mut headers = details.select(&selectors.header);
    let header = headers.next().ok_or(EquipmentParseError::PageStructure)?;
    if headers.next().is_some() {
        return Err(EquipmentParseError::PageStructure);
    }
    let header = normalize_text(header.text().collect::<String>());
    let stats = parse_stats(details, selectors)?;
    Ok(EquipmentPageFields { name, header, stats })
}

fn parse_stats(
    panel: ElementRef<'_>,
    selectors: &EquipmentSelectors,
) -> Result<BTreeMap<String, EquipmentStat>, EquipmentParseError> {
    if !panel.value().classes().any(|class| class == "eq")
        && panel.select(&selectors.equipment).next().is_none()
    {
        return Err(EquipmentParseError::PageStructure);
    }
    let mut stats = BTreeMap::new();
    // 主属性只允许来自 `.eq` 的直接子项；`.ex`/`.ep` 内的 title 属于分组属性，必须由
    // 下方行解析器处理，否则会把 `Attack Accuracy +32.16` 误当成值在前的主属性。
    for element in panel.select(&selectors.main) {
        if element.value().classes().any(|class| matches!(class, "ex" | "ep")) {
            continue;
        }
        let text = normalize_text(element.text().collect::<String>());
        let title = element.value().attr("title");
        // 普通说明或锻造提示可有自己的 title；只有 Base 声明或值在前的文本才是主属性。
        // 已识别主属性缺失/损坏 Base 不能被当成可选说明静默丢弃。
        if !declares_base(title) && !starts_stat_value(&text) {
            continue;
        }
        let base = title.and_then(parse_base).ok_or(EquipmentParseError::InvalidAttributes)?;
        let (name, value) = parse_main_stat(&text).ok_or(EquipmentParseError::InvalidAttributes)?;
        insert_stat(&mut stats, name, value, Some(base))?;
    }
    for group in panel.select(&selectors.groups) {
        let mut rows = group.children().filter_map(ElementRef::wrap);
        let group_name = if group.value().classes().any(|class| class == "ep") {
            let heading = rows.next().ok_or(EquipmentParseError::InvalidAttributes)?;
            let name = normalize_text(heading.text().collect::<String>());
            if heading.children().any(|child| ElementRef::wrap(child).is_some())
                || !valid_text(&name, 128)
                || declares_base(heading.value().attr("title"))
                || has_stat_marker(&name)
            {
                return Err(EquipmentParseError::InvalidAttributes);
            }
            Some(name)
        } else {
            None
        };
        for row in rows {
            let mut elements = row.children().filter_map(ElementRef::wrap);
            let name_element = elements.next();
            let value_element = elements.next();
            let title = row.value().attr("title");
            // 无分组说明可省略；分组标题之后的属性行不能缺列、单位或声明的 Base。
            if group_name.is_none()
                && value_element.is_none()
                && !declares_base(title)
                && !has_stat_marker(&normalize_text(row.text().collect::<String>()))
            {
                continue;
            }
            let (Some(name_element), Some(value_element)) = (name_element, value_element) else {
                return Err(EquipmentParseError::InvalidAttributes);
            };
            let unit_element = elements.next();
            if elements.next().is_some() {
                return Err(EquipmentParseError::InvalidAttributes);
            }
            let name = normalize_text(name_element.text().collect::<String>());
            let value =
                normalize_stat_value(&normalize_text(value_element.text().collect::<String>()))
                    .ok_or(EquipmentParseError::InvalidAttributes)?;
            if let Some(unit_element) = unit_element {
                let unit = normalize_text(unit_element.text().collect::<String>());
                if unit != "%" && unit != "x" {
                    return Err(EquipmentParseError::InvalidAttributes);
                }
            }
            let base = if declares_base(title) {
                Some(title.and_then(parse_base).ok_or(EquipmentParseError::InvalidAttributes)?)
            } else {
                None
            };
            // 元素名在法伤与减伤等分组内可重名；只有同一完整属性键重复才拒绝。
            let name = match &group_name {
                Some(group_name) => format!("{group_name} / {name}"),
                None => name,
            };
            insert_stat(&mut stats, name, value, base)?;
        }
    }
    if stats.is_empty() {
        return Err(EquipmentParseError::InvalidAttributes);
    }
    Ok(stats)
}

fn declares_base(title: Option<&str>) -> bool {
    title.is_some_and(|title| title.trim().starts_with("Base:"))
}

fn has_stat_marker(text: &str) -> bool {
    text.split_whitespace().any(starts_stat_value)
}

fn starts_stat_value(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'+' | b'-'))
        && bytes.next().is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
}

fn insert_stat(
    stats: &mut BTreeMap<String, EquipmentStat>,
    name: String,
    value: String,
    base: Option<f64>,
) -> Result<(), EquipmentParseError> {
    if !valid_text(&name, 128) || stats.insert(name, EquipmentStat { value, base }).is_some() {
        return Err(EquipmentParseError::InvalidAttributes);
    }
    if stats.len() > 256 {
        return Err(EquipmentParseError::InvalidAttributes);
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

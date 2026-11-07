//! 论坛首帖目录与拍品字段解析。

use std::collections::HashSet;

use auction_application::admin::AdminForumLot;

use super::commands::parse_catalog_amount;
use super::{CATALOG_CATEGORIES, CatalogParse, DEFAULT_START_PRICE};

pub(super) fn parse_catalog(input: &str) -> Result<CatalogParse, ()> {
    let mut categories = Vec::new();
    let mut category_set = HashSet::new();
    let mut lots = Vec::new();
    let mut lot_set = HashSet::new();
    let mut warnings = Vec::new();
    let mut category = None::<String>;

    for (line_index, line) in super::text::normalized_lines(input).enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = category_heading(trimmed) {
            if category_set.insert(name.to_owned()) {
                categories.push(name.to_owned());
            }
            category = Some(name.to_owned());
            continue;
        }
        let Some((raw_code, description)) = catalog_row(trimmed) else {
            continue;
        };
        // 首个已知分类之前是主题标题和导航链接，不属于拍品目录，不能产生伪解析警告。
        let Some(category) = category.as_deref() else {
            continue;
        };
        let Some(code) = super::helpers::normalize_lot_code(raw_code) else {
            warnings.push(super::helpers::warning(
                line_number,
                "invalid_catalog_row",
                "catalog row has no valid category or code",
                Some(trimmed),
                None,
            ));
            continue;
        };
        if !lot_set.insert(code.clone()) {
            return Err(());
        }
        // ASCII 小写不改变 UTF-8 字节边界；字段探测共用一份副本，展示仍切取原文。
        let lower_description = description.to_ascii_lowercase();
        let start_price = match custom_start_price(&lower_description) {
            Some(Ok(value)) => value,
            Some(Err(())) => {
                warnings.push(super::helpers::warning(
                    line_number,
                    "invalid_start_price",
                    "invalid custom start price; default used",
                    Some(trimmed),
                    None,
                ));
                DEFAULT_START_PRICE
            }
            None => DEFAULT_START_PRICE,
        };
        let (seller, current_bidder, current_price) =
            seller_and_bid(description, &lower_description);
        lots.push(AdminForumLot {
            code,
            category: category.to_owned(),
            description: description.to_owned(),
            seller,
            start_price,
            material: is_material(&lower_description),
            current_bidder,
            current_price,
        });
    }
    if lots.is_empty() {
        return Err(());
    }
    Ok(((categories, lots), warnings))
}

fn catalog_row(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let code = &rest[..end];
    Some((code, rest[end + 1..].trim()))
}

fn category_heading(value: &str) -> Option<&'static str> {
    match value {
        "Light Armor" => Some("Light"),
        "Heavy Armor" => Some("Heavy"),
        _ => CATALOG_CATEGORIES.iter().copied().find(|category| *category == value),
    }
}

fn custom_start_price(lower: &str) -> Option<Result<i64, ()>> {
    let start = lower.find("start")?;
    let tail = lower[start + "start".len()..].trim_start();
    let tail = tail.strip_prefix(':')?.trim_start();
    let token = tail
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| matches!(character, ',' | ';' | ')' | ']'));
    // 起拍价是目录事实，可为任意正整数；只有实际竞价命令需要满足 credit 网格。
    Some(parse_catalog_amount(token))
}

fn seller_and_bid(description: &str, lower: &str) -> (Option<String>, Option<String>, Option<i64>) {
    let Some(start) = lower.find("(seller:") else {
        return (None, None, None);
    };
    let content_start = start + "(seller:".len();
    let Some(end_offset) = description[content_start..].find(')') else {
        return (None, None, None);
    };
    let end = content_start + end_offset;
    let seller = description[content_start..end].trim().to_owned();
    let tail = description[end + 1..].trim();
    let lower_tail = lower[end + 1..].trim();
    if let Some(current_start) = lower_tail.find("| current:") {
        let current = tail[current_start + "| current:".len()..].trim_start();
        let lower_current = lower_tail[current_start + "| current:".len()..].trim_start();
        let amount = current
            .split_whitespace()
            .next()
            .map(|value| value.trim_matches(|character: char| matches!(character, ',' | ';')))
            .and_then(|value| parse_catalog_amount(value).ok());
        let bidder = lower_current
            .find(" by ")
            .map(|start| {
                let bidder = current[start + " by ".len()..]
                    .split_once(" | ")
                    .map_or(&current[start + " by ".len()..], |(bidder, _)| bidder)
                    .trim();
                strip_bid_source_suffix(strip_settlement_suffix(bidder)).to_owned()
            })
            .filter(|value| !value.is_empty());
        return ((!seller.is_empty()).then_some(seller), bidder, amount);
    }
    if lower_tail.contains("start:") {
        return ((!seller.is_empty()).then_some(seller), None, None);
    }
    let fields = tail.split_whitespace().collect::<Vec<_>>();
    let mut bidder = None;
    let mut price = None;
    for index in 0..fields.len() {
        if let Ok(value) = parse_catalog_amount(fields[index]) {
            if index > 0 {
                bidder = Some(fields[..index].join(" "));
            }
            price = Some(value);
            break;
        }
    }
    ((!seller.is_empty()).then_some(seller), bidder, price)
}

/// 仅识别目录渲染器的终态尾注；不放宽用户竞价命令或金额解析。
fn strip_settlement_suffix(value: &str) -> &str {
    let Some((bidder, annotation)) = value.rsplit_once("; winning for ") else {
        return value;
    };
    let Some((hours, adjustment)) = annotation.split_once("; ") else { return value };
    let Some((held, planned)) = hours.split_once("h / ") else { return value };
    let Some(planned) = planned.strip_suffix('h') else { return value };
    let valid_hours = |value: &str| {
        value.split_once('.').is_some_and(|(whole, fraction)| {
            !whole.is_empty()
                && whole.bytes().all(|byte| byte.is_ascii_digit())
                && fraction.len() == 1
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
    };
    let valid_adjustment = adjustment.strip_suffix('%').is_some_and(|value| {
        if value == "0" {
            return true;
        }
        let Some(value) = value.strip_prefix('+').or_else(|| value.strip_prefix('-')) else {
            return false;
        };
        let (whole, fraction) = value
            .split_once('.')
            .map_or((value, None), |(whole, fraction)| (whole, Some(fraction)));
        !whole.is_empty()
            && whole.bytes().all(|byte| byte.is_ascii_digit())
            && fraction.is_none_or(|fraction| {
                !fraction.is_empty()
                    && fraction.len() <= 4
                    && fraction.bytes().all(|byte| byte.is_ascii_digit())
            })
    });
    if valid_hours(held) && valid_hours(planned) && valid_adjustment { bidder } else { value }
}

/// 固定目录的来源标记只属于展示，不得污染回读后的玩家名称。
fn strip_bid_source_suffix(value: &str) -> &str {
    // 新版使用英文半角来源标记；旧帖的全角中文标记仍只读兼容，不得混入玩家名称。
    let suffix = value
        .strip_suffix(')')
        .and_then(|value| value.rsplit_once(" ("))
        .or_else(|| value.strip_suffix('）').and_then(|value| value.rsplit_once('（')));
    let Some((bidder, annotation)) = suffix else {
        return value;
    };
    let forum_floor = annotation
        .strip_prefix('#')
        .and_then(|floor| floor.parse::<i32>().ok())
        .is_some_and(|floor| floor >= 3);
    if matches!(annotation, "proxy" | "代理") || forum_floor { bidder.trim_end() } else { value }
}

fn is_material(lower: &str) -> bool {
    lower.contains("[mat]")
        || lower
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == "mat" || word == "material")
}

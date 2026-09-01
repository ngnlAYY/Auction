//! 论坛首帖目录与拍品字段解析。

use std::collections::HashSet;

use auction_application::AdminForumLot;

use super::commands::parse_amount;
use super::{CATALOG_CATEGORIES, CatalogParse, DEFAULT_START_PRICE};

pub(super) fn parse_catalog(input: &str) -> Result<CatalogParse, ()> {
    let mut categories = Vec::new();
    let mut category_set = HashSet::new();
    let mut lots = Vec::new();
    let mut lot_set = HashSet::new();
    let mut warnings = Vec::new();
    let mut category = None::<String>;

    for (line_index, line) in super::text::normalized_lines(input).into_iter().enumerate() {
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
        let Some(code) = normalize_code(raw_code) else {
            warnings.push(super::helpers::warning(
                line_number,
                "invalid_catalog_row",
                "catalog row has no valid category or code",
                Some(trimmed),
                None,
            ));
            continue;
        };
        let Some(category) = category.as_deref() else {
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
        let start_price = match custom_start_price(description) {
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
        let (seller, current_bidder, current_price) = seller_and_bid(description);
        lots.push(AdminForumLot {
            code,
            category: category.to_owned(),
            description: description.to_owned(),
            seller,
            start_price,
            material: is_material(description),
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

fn normalize_code(raw: &str) -> Option<String> {
    let value = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .replace('-', "")
        .to_ascii_uppercase();
    if value.len() != 5
        || !value.is_ascii()
        || !value.as_bytes()[..3].iter().all(u8::is_ascii_uppercase)
        || !value.as_bytes()[3..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(format!("{}{}{}", &value[0..1], value[1..3].to_ascii_lowercase(), &value[3..]))
}

fn custom_start_price(description: &str) -> Option<Result<i64, ()>> {
    let lower = description.to_ascii_lowercase();
    let start = lower.find("start")?;
    let tail = lower[start + "start".len()..].trim_start();
    let tail = tail.strip_prefix(':')?.trim_start();
    let token = tail
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| matches!(character, ',' | ';' | ')' | ']'));
    Some(parse_amount(token))
}

fn seller_and_bid(description: &str) -> (Option<String>, Option<String>, Option<i64>) {
    let lower = description.to_ascii_lowercase();
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
    let lower_tail = tail.to_ascii_lowercase();
    if let Some(current_start) = lower_tail.find("| current:") {
        let current = tail[current_start + "| current:".len()..].trim_start();
        let amount = current
            .split_whitespace()
            .next()
            .map(|value| value.trim_matches(|character: char| matches!(character, ',' | ';')))
            .and_then(|value| parse_amount(value).ok());
        let bidder = current
            .to_ascii_lowercase()
            .find(" by ")
            .map(|start| {
                current[start + " by ".len()..]
                    .split_once(" | ")
                    .map_or(&current[start + " by ".len()..], |(bidder, _)| bidder)
                    .trim()
                    .to_owned()
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
        if let Ok(value) = parse_amount(fields[index]) {
            if index > 0 {
                bidder = Some(fields[..index].join(" "));
            }
            price = Some(value);
            break;
        }
    }
    ((!seller.is_empty()).then_some(seller), bidder, price)
}

fn is_material(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    lower.contains("[mat]")
        || lower
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == "mat" || word == "material")
}

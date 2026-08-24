//! 装备名称、品质、前缀、类型和后缀的身份映射。

use auction_application::AdminIntakeError;

use super::common::valid_optional_text;

pub(crate) fn parse_identity(
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

pub(crate) fn category_for_type(value: &str) -> Option<String> {
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

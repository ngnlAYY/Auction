//! MoogleMail HTML/JavaScript 中的装备 key 提取与严格 JSON 访问。

use super::*;

pub(crate) fn equipment_keys(
    document: &Html,
    body: &str,
    wanted: &HashSet<String>,
    world: MoogleMailWorld,
) -> Result<HashMap<String, MoogleMailEquipmentKey>, MoogleMailError> {
    let mut keys = body_equipment_keys(body, wanted, world)?;
    let mut script_keys = HashMap::new();
    let script_selector = selector("script", MoogleMailOperation::Detail, world)?;
    for script in document.select(&script_selector) {
        let source = script.text().collect::<String>();
        let Some(masked) = executable_javascript(&source) else {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        };
        let starts = masked.match_indices('{').map(|(index, _)| index).collect::<Vec<_>>();
        for start in starts {
            let candidate = &source[start..];
            if !wanted.iter().any(|equipment_id| has_json_property(candidate, equipment_id)) {
                continue;
            }
            let (found, saw_wanted) = decode_equipment_root(candidate, wanted)
                .map_err(|_| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if !saw_wanted {
                continue;
            }
            for (equipment_id, key) in found {
                if script_keys.insert(equipment_id.clone(), key).is_some() {
                    return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
                }
            }
        }
    }
    for (equipment_id, key) in script_keys {
        if let Some(body_key) = keys.get(&equipment_id) {
            if body_key != &key {
                return Err(detail_error(world, MoogleMailErrorCategory::IdentityMismatch));
            }
        } else {
            keys.insert(equipment_id, key);
        }
    }
    if keys.len() != wanted.len() || wanted.iter().any(|id| !keys.contains_key(id)) {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(keys)
}

pub(crate) fn body_equipment_keys(
    body: &str,
    wanted: &HashSet<String>,
    world: MoogleMailWorld,
) -> Result<HashMap<String, MoogleMailEquipmentKey>, MoogleMailError> {
    let mut keys = HashMap::new();
    let mut offset = 0;
    while let Some(relative) = body[offset..].find("/equip/") {
        let start = offset + relative + "/equip/".len();
        let mut index = start;
        while body.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_digit()) {
            index += 1;
        }
        if index == start || body.as_bytes().get(index) != Some(&b'/') {
            offset = start;
            continue;
        }
        let equipment_id = &body[start..index];
        let key_start = index + 1;
        let key_end = key_start.saturating_add(10);
        if key_end > body.len() || !valid_hex_key(&body[key_start..key_end]) {
            offset = key_start;
            continue;
        }
        if body.as_bytes().get(key_end).is_some_and(|byte| byte.is_ascii_hexdigit()) {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if wanted.contains(equipment_id) {
            let key = MoogleMailEquipmentKey::new(&body[key_start..key_end])
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if keys.insert(equipment_id.to_owned(), key).is_some() {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
        }
        offset = key_end;
    }
    Ok(keys)
}

pub(crate) fn has_json_property(source: &str, name: &str) -> bool {
    let needle = format!("\"{name}\"");
    let mut offset = 0;
    while offset < source.len() {
        let Some(relative) = source[offset..].find(&needle) else {
            return false;
        };
        let mut index = offset + relative + needle.len();
        while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
            index += 1;
        }
        if source.as_bytes().get(index) == Some(&b':') {
            return true;
        }
        offset = index;
    }
    false
}

pub(crate) fn decode_equipment_root(
    source: &str,
    wanted: &HashSet<String>,
) -> Result<(HashMap<String, MoogleMailEquipmentKey>, bool), ()> {
    let mut decoder = serde_json::Deserializer::from_str(source);
    decoder.deserialize_map(EquipmentRootVisitor { wanted }).map_err(|_| ())
}

pub(crate) struct EquipmentRootVisitor<'a> {
    wanted: &'a HashSet<String>,
}

impl<'de> Visitor<'de> for EquipmentRootVisitor<'_> {
    type Value = (HashMap<String, MoogleMailEquipmentKey>, bool);

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MoogleMail equipment JSON object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashMap::new();
        while let Some(name) = map.next_key::<String>()? {
            if self.wanted.contains(&name) {
                if keys.contains_key(&name) {
                    return Err(de::Error::custom("重复装备 JSON 字段"));
                }
                let item = map.next_value::<EquipmentJsonItem>()?;
                keys.insert(name, item.0);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        let saw_wanted = !keys.is_empty();
        Ok((keys, saw_wanted))
    }
}

pub(crate) struct EquipmentJsonItem(pub(crate) MoogleMailEquipmentKey);

pub(crate) struct EquipmentJsonItemVisitor;

impl<'de> Visitor<'de> for EquipmentJsonItemVisitor {
    type Value = EquipmentJsonItem;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("包含唯一 k 字段的装备对象")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut key = None;
        while let Some(name) = map.next_key::<String>()? {
            if name == "k" {
                if key.is_some() {
                    return Err(de::Error::custom("重复装备 key 字段"));
                }
                let value = map.next_value::<String>()?;
                let parsed = MoogleMailEquipmentKey::new(&value)
                    .ok_or_else(|| de::Error::custom("装备 key 格式无效"))?;
                key = Some(parsed);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        key.map(EquipmentJsonItem).ok_or_else(|| de::Error::custom("装备对象缺少 key 字段"))
    }
}

impl<'de> serde::Deserialize<'de> for EquipmentJsonItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(EquipmentJsonItemVisitor)
    }
}

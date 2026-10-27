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
        // 只解析目标字段所属的最内层对象；外层 JavaScript 函数体不是 JSON。
        // 目标位置按遮罩后的括号栈定位，字段和对象仍由严格 JSON 解码器验证。
        for start in equipment_root_candidates(&source, &masked, wanted) {
            let candidate = &source[start..];
            let (found, saw_wanted) = decode_equipment_root(candidate, wanted)
                .map_err(|_| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if !saw_wanted {
                continue;
            }
            for (equipment_id, key) in found {
                if script_keys.insert(equipment_id, key).is_some() {
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
    // 非装备邮件无需扫描正文；避免发送者在正文构造畸形 /equip 链接阻断整个收件轮次。
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
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
        // 正文中的其它装备链接不是附件证据；其长度、编码和 key 均不能阻断当前附件读取。
        if !wanted.contains(equipment_id) {
            offset = key_start;
            continue;
        }
        let key_end = key_start.saturating_add(10);
        // 邮件正文允许 Unicode；固定长度 key 必须先验证切片边界，不能因无效链接使 Worker panic。
        let Some(raw_key) = body.get(key_start..key_end).filter(|value| valid_hex_key(value))
        else {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        };
        if body.as_bytes().get(key_end).is_some_and(|byte| byte.is_ascii_hexdigit()) {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let key = MoogleMailEquipmentKey::new(raw_key)
            .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if keys.insert(equipment_id.to_owned(), key).is_some() {
            return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        offset = key_end;
    }
    Ok(keys)
}

fn equipment_root_candidates(source: &str, masked: &str, wanted: &HashSet<String>) -> Vec<usize> {
    let source_bytes = source.as_bytes();
    let masked_bytes = masked.as_bytes();
    let mut open_braces = Vec::new();
    let mut candidates = Vec::new();
    for (start, masked_byte) in masked_bytes.iter().copied().enumerate() {
        match masked_byte {
            b'{' => open_braces.push(start),
            b'}' => {
                open_braces.pop();
            }
            _ if source_bytes[start] == b'"' => {
                let mut end = start + 1;
                while source_bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
                if end == start + 1 || source_bytes.get(end) != Some(&b'"') {
                    continue;
                }
                let Some(id) = source.get(start + 1..end) else { continue };
                if !wanted.contains(id) {
                    continue;
                }
                let mut colon = end + 1;
                while source_bytes.get(colon).is_some_and(u8::is_ascii_whitespace) {
                    colon += 1;
                }
                // 字符串和注释内的伪字段，其冒号也会被遮罩。
                if masked_bytes.get(colon) == Some(&b':')
                    && let Some(&object_start) = open_braces.last()
                {
                    candidates.push(object_start);
                }
            }
            _ => {}
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    candidates
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

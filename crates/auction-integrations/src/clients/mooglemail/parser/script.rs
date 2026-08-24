use super::*;

pub(crate) fn required_script_value(source: &str, name: &str) -> Option<i64> {
    let needle = format!("var {name}");
    let mut matches = source.match_indices(&needle);
    let first = matches.find(|(index, _)| {
        *index == 0
            || source[..*index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_whitespace() || character == ';')
    })?;
    if matches.next().is_some() {
        return None;
    }
    let mut index = first.0 + needle.len();
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    if source.as_bytes().get(index) != Some(&b'=') {
        return None;
    }
    index += 1;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    let start = index;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_digit()) {
        index += 1;
    }
    if start == index {
        return None;
    }
    let value = source[start..index].parse::<i64>().ok()?;
    while source.as_bytes().get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
        index += 1;
    }
    (source.as_bytes().get(index) == Some(&b';')).then_some(value)
}

pub(crate) fn script_text(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<String, MoogleMailError> {
    let selector = selector("script", operation, world)?;
    let mut source = String::new();
    for script in document.select(&selector) {
        source.push_str(&script.text().collect::<String>());
        source.push('\n');
    }
    Ok(source)
}

/// 把注释和字符串替换为空格，防止伪造的变量赋值或函数调用进入协议状态。
pub(crate) fn executable_javascript(source: &str) -> Option<String> {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment,
    }
    let mut state = State::Code;
    let mut escaped = false;
    let mut output = source.as_bytes().to_vec();
    let mut index = 0;
    while index < output.len() {
        let byte = source.as_bytes()[index];
        match state {
            State::Code => match byte {
                b'/' if source.as_bytes().get(index + 1) == Some(&b'/') => {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::LineComment;
                }
                b'/' if source.as_bytes().get(index + 1) == Some(&b'*') => {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::BlockComment;
                }
                b'\'' => {
                    output[index] = b' ';
                    state = State::Single;
                }
                b'"' => {
                    output[index] = b' ';
                    state = State::Double;
                }
                b'`' => {
                    output[index] = b' ';
                    state = State::Template;
                }
                _ => {}
            },
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Code;
                } else {
                    output[index] = b' ';
                }
            }
            State::BlockComment => {
                if byte == b'*' && source.as_bytes().get(index + 1) == Some(&b'/') {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 1;
                    state = State::Code;
                } else if byte != b'\n' && byte != b'\r' {
                    output[index] = b' ';
                }
            }
            State::Single | State::Double | State::Template => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if matches!(
                    (state, byte),
                    (State::Single, b'\'') | (State::Double, b'"') | (State::Template, b'`')
                ) {
                    state = State::Code;
                } else if (byte == b'\n' || byte == b'\r') && !matches!(state, State::Template) {
                    return None;
                }
                if !matches!(byte, b'\n' | b'\r') {
                    output[index] = b' ';
                }
            }
        }
        index += 1;
    }
    matches!(state, State::Code | State::LineComment)
        .then(|| String::from_utf8(output).ok())
        .flatten()
}

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

pub(crate) struct EquipmentJsonItem(MoogleMailEquipmentKey);

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

pub(crate) fn extract_equipment_id(value: &str) -> Option<String> {
    let marker = "equips.set(";
    let matches = value.match_indices(marker).take(2).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let (marker_start, _) = matches[0];
    if marker_start > 0
        && !value[..marker_start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_whitespace() || character == ';')
    {
        return None;
    }
    let start = marker_start + marker.len();
    let remainder = value[start..].trim_start();
    let end = remainder.find(|character: char| !character.is_ascii_digit())?;
    let id = &remainder[..end];
    (valid_decimal_id(id) && remainder[end..].trim_start().starts_with(',')).then(|| id.to_owned())
}

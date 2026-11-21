//! 在保留原始数字 token 的同时，遍历校验整个 JSON 的边界和键唯一性。

use std::collections::BTreeMap;
use std::fmt;

use auction_application::consignment::LookupPermanentReason;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

type Reason = LookupPermanentReason;
const MAX_DEPTH: usize = 8;
const MAX_OBJECT_KEYS: usize = 128;
const MAX_ARRAY_ITEMS: usize = 256;

pub(super) type Object<'a> = BTreeMap<String, Value<'a>>;

pub(super) enum Value<'a> {
    Object(Object<'a>),
    Array(Vec<Self>),
    Text(String),
    Number(&'a str),
    Bool(bool),
    Null,
}

impl Value<'_> {
    pub(super) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    pub(super) fn object(&self) -> Option<&Object<'_>> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    pub(super) fn number(&self) -> Option<&str> {
        match self {
            Self::Number(token) => Some(token),
            _ => None,
        }
    }
}

/// 先让 serde 检查完整文法，再按原 token 逐层校验结构，避免普通 Value 覆盖重复键或舍入 EID。
pub(super) fn parse_object(body: &[u8]) -> Result<Object<'_>, Reason> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Err(Reason::SourceSchemaInvalid);
    };
    let Ok(raw) = serde_json::from_str::<&RawValue>(text) else {
        return Err(Reason::SourceSchemaInvalid);
    };
    match value(raw, 1, false)? {
        Value::Object(object) => Ok(object),
        _ => Err(Reason::SourceSchemaInvalid),
    }
}

fn value(raw: &RawValue, depth: usize, is_eid: bool) -> Result<Value<'_>, Reason> {
    let token = raw.get();
    let mut failure = Reason::SourceSchemaInvalid;
    let mut deserializer = serde_json::Deserializer::from_str(token);
    let visitor = Container { depth, failure: &mut failure };
    let parsed = match token.as_bytes().first() {
        Some(b'{') => serde::Deserializer::deserialize_map(&mut deserializer, visitor),
        Some(b'[') => serde::Deserializer::deserialize_seq(&mut deserializer, visitor),
        Some(b'"') => serde_json::from_str(token).map(Value::Text),
        Some(b't') => return Ok(Value::Bool(true)),
        Some(b'f') => return Ok(Value::Bool(false)),
        Some(b'n') => return Ok(Value::Null),
        Some(b'-' | b'0'..=b'9') => {
            // EID 的位数和词法由身份校验直接比较，不从浮点数反推。
            if !is_eid {
                finite_number(token)?;
            }
            return Ok(Value::Number(token));
        }
        _ => return Err(Reason::SourceSchemaInvalid),
    };
    // JSON 解码错误可能含字段原文，只保留结构校验分类。
    let Ok(value) = parsed else {
        return Err(failure);
    };
    Ok(value)
}

pub(super) fn finite_number(token: &str) -> Result<f64, Reason> {
    let Ok(number) = token.parse::<f64>() else {
        return Err(Reason::SourceSchemaInvalid);
    };
    let significand = token.split(['e', 'E']).next().unwrap_or_default();
    if !number.is_finite()
        || (number == 0.0 && significand.bytes().any(|byte| matches!(byte, b'1'..=b'9')))
    {
        return Err(Reason::SourceSchemaInvalid);
    }
    Ok(number)
}

struct Container<'a> {
    depth: usize,
    failure: &'a mut Reason,
}

impl Container<'_> {
    fn reject<E: serde::de::Error>(&mut self, reason: Reason) -> E {
        *self.failure = reason;
        E::custom("装备来源 JSON 边界无效")
    }
}

impl<'de> Visitor<'de> for Container<'_> {
    type Value = Value<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("装备来源 JSON 容器")
    }

    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<Self::Value, A::Error> {
        if self.depth > MAX_DEPTH {
            return Err(self.reject(Reason::SourceTooLarge));
        }
        let mut object = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(self.reject(Reason::SourceSchemaInvalid));
            }
            if object.len() == MAX_OBJECT_KEYS {
                return Err(self.reject(Reason::SourceTooLarge));
            }
            let raw: &RawValue = map.next_value()?;
            let item = value(raw, self.depth + 1, self.depth == 1 && key == "eid")
                .map_err(|reason| self.reject(reason))?;
            object.insert(key, item);
        }
        Ok(Value::Object(object))
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<Self::Value, A::Error> {
        if self.depth > MAX_DEPTH {
            return Err(self.reject(Reason::SourceTooLarge));
        }
        let mut items = Vec::new();
        while let Some(raw) = seq.next_element::<&RawValue>()? {
            if items.len() == MAX_ARRAY_ITEMS {
                return Err(self.reject(Reason::SourceTooLarge));
            }
            let item = value(raw, self.depth + 1, false).map_err(|reason| self.reject(reason))?;
            items.push(item);
        }
        Ok(Value::Array(items))
    }
}

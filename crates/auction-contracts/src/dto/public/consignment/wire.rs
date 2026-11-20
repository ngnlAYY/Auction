//! 寄售 DTO 的 JSON 对象解码和共享标量边界。

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde::de::Error;
use uuid::Uuid;

pub(in crate::dto) const SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;
pub(super) const PRICE_MAX: i64 = 1_000_000_000;
pub(super) const FEE_BPS_MAX: i64 = 9_999;

// Serde 的结构体默认也接受位置数组。用 MapAccess 保留重复键与原始数值形态，
// 再交给派生解码检查闭集；经 Value 中转会丢掉重复键。
macro_rules! json_object {
    (
        ($($serde_options:tt)*)
        $(#[$attr:meta])*
        pub $kind:ident $name:ident $(<$item:ident>)? { $($body:tt)* }
        $(=> $validate:ident)?
    ) => {
        $(#[$attr])*
        #[derive(serde::Serialize)]
        #[serde($($serde_options)*)]
        pub $kind $name $(<$item>)? { $($body)* }

        impl<'de $(, $item: serde::Deserialize<'de>)?> serde::Deserialize<'de>
            for $name $(<$item>)?
        {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                type Dto $(<$item>)? = $name $(<$item>)?;

                #[derive(serde::Deserialize)]
                #[serde(remote = "Dto", deny_unknown_fields, $($serde_options)*)]
                $kind Wire $(<$item>)? { $($body)* }

                struct ObjectVisitor<T>(std::marker::PhantomData<T>);

                impl<'de $(, $item: serde::Deserialize<'de>)?> serde::de::Visitor<'de>
                    for ObjectVisitor<$name $(<$item>)?>
                {
                    type Value = $name $(<$item>)?;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("寄售 JSON 对象")
                    }

                    fn visit_map<M: serde::de::MapAccess<'de>>(
                        self, map: M,
                    ) -> Result<Self::Value, M::Error> {
                        let value = Wire::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                        $(value.$validate::<M::Error>()?;)?
                        Ok(value)
                    }
                }

                deserializer.deserialize_map(ObjectVisitor(std::marker::PhantomData))
            }
        }
    };
}

pub(in crate::dto) use json_object;

pub(in crate::dto) fn integer<'de, const MIN: i64, const MAX: i64, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<i64, D::Error> {
    let value = i64::deserialize(deserializer)?;
    if (MIN..=MAX).contains(&value) {
        return Ok(value);
    }
    Err(D::Error::custom("寄售整数超出契约范围"))
}

pub(super) fn optional_integer<'de, const MIN: i64, const MAX: i64, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Bounded<const MIN: i64, const MAX: i64>(
        #[serde(deserialize_with = "integer::<MIN, MAX, _>")] i64,
    );

    Option::<Bounded<MIN, MAX>>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

pub(in crate::dto) fn uuid<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
    let text = String::deserialize(deserializer)?;
    let uuid = Uuid::parse_str(&text).map_err(D::Error::custom)?;
    let mut buffer = Uuid::encode_buffer();
    let canonical: &str = uuid.hyphenated().encode_lower(&mut buffer);
    if !uuid.is_nil() && canonical == text {
        return Ok(uuid);
    }
    Err(D::Error::custom("寄售标识必须为规范小写的非零 UUID"))
}

pub(super) fn optional_uuid<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Canonical(#[serde(deserialize_with = "uuid")] Uuid);

    Option::<Canonical>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

pub(in crate::dto) fn timestamp<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Timestamp, D::Error> {
    let text = String::deserialize(deserializer)?;
    // 既有 Timestamp 的宽松输入含非 UTC 偏移和空格。此契约只接受 RFC 3339 UTC。
    if !matches!(text.as_bytes().get(10), Some(b'T' | b't'))
        || !(text.ends_with(['Z', 'z']) || text.ends_with("+00:00"))
    {
        return Err(D::Error::custom("寄售时刻必须为 RFC 3339 UTC"));
    }
    Timestamp::parse_from_rfc3339(&text).map_err(D::Error::custom)
}

pub(super) fn optional_timestamp<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Timestamp>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Utc(#[serde(deserialize_with = "timestamp")] Timestamp);

    Option::<Utc>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

pub(super) fn equipment_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    let bytes = text.as_bytes();
    if !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
    {
        return Ok(text);
    }
    Err(D::Error::custom("外部装备编号必须为规范正十进制字符串"))
}

pub(super) fn unavailable<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    if bool::deserialize(deserializer)? {
        return Err(D::Error::custom("本期寄售能力字段必须为 false"));
    }
    Ok(false)
}

pub(super) fn serialize_unavailable<S: Serializer>(
    value: &bool,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if *value {
        return Err(serde::ser::Error::custom("本期寄售能力字段必须为 false"));
    }
    serializer.serialize_bool(false)
}

pub(in crate::dto) fn text<'de, const MAX: usize, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    checked_text::<MAX, D::Error>(String::deserialize(deserializer)?)
}

pub(in crate::dto) fn optional_text<'de, const MAX: usize, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)?.map(checked_text::<MAX, D::Error>).transpose()
}

fn checked_text<const MAX: usize, E: Error>(text: String) -> Result<String, E> {
    if !text.is_empty()
        && text.len() <= MAX
        && text == text.trim()
        && !text.chars().any(char::is_control)
    {
        return Ok(text);
    }
    Err(E::custom("寄售文本必须为规范单行且在字节上限内"))
}

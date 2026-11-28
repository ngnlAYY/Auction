//! 寄售 DTO 的 JSON 对象解码和共享标量边界。

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde::de::Error;
use uuid::Uuid;

pub(super) const SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;
pub(super) const PRICE_MAX: i64 = 1_000_000_000;
pub(super) const FEE_BPS_MAX: i64 = 9_999;

// 用 MapAccess 拒绝位置数组并保留重复键与原始数值形态。两个方向共用校验，
// 直接构造 Rust DTO 也不能绕过响应投影的不变量。
macro_rules! json_object {
    (
        ($($options:tt)*)
        $(#[$attr:meta])*
        pub struct $name:ident $(<$item:ident>)? {
            $($(#[$field_attr:meta])* pub $field:ident: $ty:ty,)*
        }
        $(=> $validate:ident $($direction:ident)?)?
    ) => {
        json_object!(@impl ($($options)*) ($(#[$attr])*) struct $name $(<$item>)?
            { $(pub $field: $ty,)* }
            { $($(#[$field_attr])* pub $field: $ty,)* }
            $(=> $validate $($direction)?)?
        );
    };
    (
        ($($options:tt)*)
        $(#[$attr:meta])*
        pub enum $name:ident {
            $($(#[$variant_attr:meta])* $variant:ident {
                $($(#[$field_attr:meta])* $field:ident: $ty:ty),* $(,)?
            },)*
        }
        $(=> $validate:ident $($direction:ident)?)?
    ) => {
        json_object!(@impl ($($options)*) ($(#[$attr])*) enum $name
            { $($(#[$variant_attr])* $variant { $($field: $ty,)* },)* }
            { $($variant { $($(#[$field_attr])* $field: $ty,)* },)* }
            $(=> $validate $($direction)?)?
        );
    };
    (
        @impl ($($options:tt)*) ($($attrs:tt)*) $kind:ident $name:ident $(<$item:ident>)?
        { $($public_body:tt)* }
        { $($wire_body:tt)* }
        $(=> $validate:ident $($direction:ident)?)?
    ) => {
        $($attrs)*
        pub $kind $name $(<$item>)? { $($public_body)* }

        const _: () = {
            type Dto $(<$item>)? = $name $(<$item>)?;

            #[derive(serde::Deserialize, serde::Serialize)]
            #[serde(remote = "Dto", deny_unknown_fields, $($options)*)]
            $kind Wire $(<$item>)? { $($wire_body)* }

            impl$(<$item: serde::Serialize>)? serde::Serialize for $name $(<$item>)? {
                fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    $(json_object!(@validate_output self, $validate $($direction)?);)?
                    Wire::serialize(self, serializer)
                }
            }

            impl<'de $(, $item: serde::Deserialize<'de>)?> serde::Deserialize<'de>
                for $name $(<$item>)?
            {
                fn deserialize<D: serde::Deserializer<'de>>(
                    deserializer: D,
                ) -> Result<Self, D::Error> {
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
                            $(value.$validate().map_err(serde::de::Error::custom)?;)?
                            Ok(value)
                        }
                    }

                    deserializer.deserialize_map(ObjectVisitor(std::marker::PhantomData))
                }
            }
        };
    };
    (@validate_output $object:ident, $validate:ident decode_only) => {};
    (@validate_output $object:ident, $validate:ident) => {
        $object.$validate().map_err(serde::ser::Error::custom)?;
    };
}

pub(in crate::dto) use json_object;

// 单元枚举的派生解码也接受外部标记对象。显式从字符串映射稳定的闭集码。
macro_rules! string_enum {
    ($(#[$attr:meta])* pub enum $name:ident {
        $($(#[$variant_attr:meta])* $variant:ident => $code:literal,)*
    }) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
        pub enum $name {
            $($(#[$variant_attr])* #[serde(rename = $code)] $variant,)*
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct StringVisitor;

                impl serde::de::Visitor<'_> for StringVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("寄售闭集字符串")
                    }

                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        match value {
                            $($code => Ok($name::$variant),)*
                            _ => Err(E::custom("未知的寄售字符串值")),
                        }
                    }
                }

                deserializer.deserialize_str(StringVisitor)
            }
        }
    };
}

pub(super) use string_enum;

pub(super) fn integer<'de, const MIN: i64, const MAX: i64, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<i64, D::Error> {
    checked_integer::<MIN, MAX>(i64::deserialize(deserializer)?).map_err(D::Error::custom)
}

pub(super) fn serialize_integer<const MIN: i64, const MAX: i64, S: Serializer>(
    value: &i64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer
        .serialize_i64(checked_integer::<MIN, MAX>(*value).map_err(serde::ser::Error::custom)?)
}

fn checked_integer<const MIN: i64, const MAX: i64>(value: i64) -> Result<i64, &'static str> {
    if (MIN..=MAX).contains(&value) {
        return Ok(value);
    }
    Err("寄售整数超出契约范围")
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

pub(in crate::dto) fn optional_uuid<'de, D: Deserializer<'de>>(
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
    let value = String::deserialize(deserializer)?;
    checked_text::<MAX>(&value).map_err(D::Error::custom)?;
    Ok(value)
}

pub(in crate::dto) fn optional_text<'de, const MAX: usize, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if let Some(text) = &value {
        checked_text::<MAX>(text).map_err(D::Error::custom)?;
    }
    Ok(value)
}

pub(in crate::dto) fn checked_text<const MAX: usize>(text: &str) -> Result<(), &'static str> {
    // U+2028、U+2029 不是控制字符，但在显示与换行语义上等同换行，必须与
    // `consignment_safe_observation_text` 及 ListingSellerName 保持同一单行口径。
    if !text.is_empty()
        && text.len() <= MAX
        && text == text.trim()
        && !text.chars().any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return Ok(());
    }
    Err("寄售文本必须为规范单行且在字节上限内")
}

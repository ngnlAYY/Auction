//! 第 4 期 v2 wire 契约的共享校验原语。

use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde::de::Error;
use uuid::Uuid;
use uuid::Variant;

pub(in crate::dto) use super::wire::uuid;

pub(in crate::dto) const SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;

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

pub(in crate::dto) use string_enum;

/// 玩家或 Admin 提交的命令 ID 必须是规范小写的非零 UUIDv4。
pub(in crate::dto) fn command_uuid<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Uuid, D::Error> {
    let value = uuid(deserializer)?;
    // 与应用层 `validate_listing_client_command_id` 同口径：客户端不能占用
    // 后台派生的 UUIDv5 命名空间，非 RFC 4122 变体也不是规范 UUIDv4。
    if value.get_version_num() == 4 && value.get_variant() == Variant::RFC4122 {
        return Ok(value);
    }
    Err(D::Error::custom("寄售命令 ID 必须为规范 UUIDv4"))
}

/// 键必须出现；值可以为 null，非空值仍须是规范小写的非零 UUID。
pub(in crate::dto) fn required_option_uuid<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Canonical(#[serde(deserialize_with = "uuid")] Uuid);

    Option::<Canonical>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

/// 正的安全整数版本或代次。
pub(in crate::dto) fn positive_version<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<i64, D::Error> {
    let value = i64::deserialize(deserializer)?;
    if (1..=SAFE_INTEGER_MAX).contains(&value) {
        return Ok(value);
    }
    Err(D::Error::custom("寄售版本或代次必须为正的安全整数"))
}

/// 序列化前复验正的安全整数版本或代次。
pub(in crate::dto) fn serialize_positive_version<S: Serializer>(
    value: &i64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if (1..=SAFE_INTEGER_MAX).contains(value) {
        return serializer.serialize_i64(*value);
    }
    Err(serde::ser::Error::custom("寄售版本或代次必须为正的安全整数"))
}

/// 认证 Bot 原信键中的外部邮件 ID：规范正十进制文本。
pub(in crate::dto) fn external_mid<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if is_external_mid(&value) {
        return Ok(value);
    }
    Err(D::Error::custom("邮件 MID 必须为规范正十进制字符串"))
}

/// 规范正十进制的外部邮件 ID。
pub(in crate::dto) fn is_external_mid(value: &str) -> bool {
    canonical_positive_decimal::<32>(value)
}

/// 规范正十进制的外部装备编号。
pub(in crate::dto) fn is_equipment_id(value: &str) -> bool {
    canonical_positive_decimal::<32>(value)
}

fn canonical_positive_decimal<const MAX: usize>(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

/// 寄件预约码：`C1-` 加 43 个无填充 base64url 字符。
pub(in crate::dto) fn reservation_code<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if is_reservation_code(&value) {
        return Ok(value);
    }
    Err(D::Error::custom("寄售预约码必须为 C1- 加 43 个规范 base64url 字符"))
}

/// 判断规范寄件预约码；43 字符载荷的末位只允许无尾随填充位的 16 个字符。
pub(in crate::dto) fn is_reservation_code(value: &str) -> bool {
    let Some(payload) = value.strip_prefix("C1-") else {
        return false;
    };
    if payload.len() != 43
        || !payload.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return false;
    }
    matches!(
        payload.as_bytes()[42],
        b'A' | b'E'
            | b'I'
            | b'M'
            | b'Q'
            | b'U'
            | b'Y'
            | b'c'
            | b'g'
            | b'k'
            | b'o'
            | b's'
            | b'w'
            | b'0'
            | b'4'
            | b'8'
    )
}

/// SHA-256 摘要的线表示为 64 位小写十六进制。
pub(in crate::dto) fn sha256_hex<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if is_sha256_hex(&value) {
        return Ok(value);
    }
    Err(D::Error::custom("摘要必须为 64 位小写十六进制"))
}

/// 判断 64 位小写十六进制摘要。
pub(in crate::dto) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::is_external_mid;
    use super::is_reservation_code;
    use super::is_sha256_hex;

    #[test]
    fn reservation_code_requires_canonical_tail_bits() {
        assert!(is_reservation_code(&format!("C1-{}", "A".repeat(43))));
        assert!(is_reservation_code(&format!("C1-{}", "8".repeat(43))));
        assert!(!is_reservation_code(&format!("C1-{}", "B".repeat(43))));
        assert!(!is_reservation_code(&format!("C1-{}", "=".repeat(43))));
        assert!(!is_reservation_code(&format!("C1-{}", "A".repeat(42))));
        assert!(!is_reservation_code(&format!("c1-{}", "A".repeat(43))));
    }

    #[test]
    fn decimal_and_digest_text_are_strict() {
        assert!(is_external_mid("42"));
        for value in ["", "0", "042", "４２", "42a"] {
            assert!(!is_external_mid(value), "{value}");
        }
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
    }
}

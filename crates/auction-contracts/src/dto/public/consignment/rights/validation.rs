//! 受限权利请求与响应的标量校验：游戏角色名、权利定位符、挑战码输入的形状边界。
//!
//! 这里只做形状把关；定位符的规范尾位、挑战码的字母表与编码、角色名的世界归属都由应用层与存储
//! 失败关闭。错误文本固定，不回显任何输入片段（不变量 7）。

use serde::Deserialize;
use serde::Deserializer;
use serde::de::Error;

/// 游戏角色名字符上限，与应用层 `GameIdentity` 同口径。
const GAME_IDENTITY_MAX_CHARS: usize = 64;
/// 权利定位符：`R1-` 加 43 个无填充 base64url 字符。
const LOCATOR_LEN: usize = 46;
/// 挑战码输入的字节上限（26 个字符加分组连字符），与领域解析同口径。
const CHALLENGE_CODE_INPUT_MAX: usize = 40;

/// 严格校验的游戏角色名：非空、无首尾空白、无控制字符、不超长。
pub(in crate::dto) fn game_identity_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    let valid = !value.is_empty()
        && value == value.trim()
        && value.chars().count() <= GAME_IDENTITY_MAX_CHARS
        && !value.chars().any(char::is_control);
    if valid {
        return Ok(value);
    }
    Err(D::Error::custom("游戏角色名必须为规范单行文本"))
}

/// 权利定位符的形状：`R1-` 前缀加 43 个 base64url 字符。
pub(in crate::dto) fn right_locator_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if is_right_locator_shape(&value) {
        return Ok(value);
    }
    Err(D::Error::custom("权利定位符必须为 R1- 加 43 个 base64url 字符"))
}

fn is_right_locator_shape(value: &str) -> bool {
    value.len() == LOCATOR_LEN
        && value.strip_prefix("R1-").is_some_and(|payload| {
            payload.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

/// 挑战码输入：只限长度与字符类；字母表、规范性与对错由服务端同形处理。
pub(in crate::dto) fn challenge_code_input<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    let valid = !value.is_empty()
        && value.len() <= CHALLENGE_CODE_INPUT_MAX
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        return Ok(value);
    }
    Err(D::Error::custom("挑战码输入必须为不超长的字母数字与连字符"))
}

#[cfg(test)]
mod tests {
    use super::is_right_locator_shape;

    #[test]
    fn locator_shape_requires_prefix_and_length() {
        assert!(is_right_locator_shape(&format!("R1-{}", "A".repeat(43))));
        assert!(!is_right_locator_shape(&format!("S1-{}", "A".repeat(43))));
        assert!(!is_right_locator_shape(&format!("R1-{}", "A".repeat(42))));
        assert!(!is_right_locator_shape(&format!("R1-{}=", "A".repeat(42))));
    }
}

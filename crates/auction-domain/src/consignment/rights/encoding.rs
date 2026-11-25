//! 带前缀的 32 字节随机量文本编码：`<前缀>` + 43 字符无填充 base64url。
//!
//! 定位符与会话令牌共用；解析后重新编码逐字比较，拒绝一切非唯一编码。

use super::errors::TextFault;

/// 随机量字节数。
pub(super) const RANDOM_BYTES: usize = 32;
/// 正文字符数。
const BODY_LEN: usize = 43;
/// 前缀字符数；两种前缀等长。
pub(super) const PREFIX_LEN: usize = 3;
/// 完整文本字节数。
pub(super) const TEXT_LEN: usize = PREFIX_LEN + BODY_LEN;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 把随机量编码为带前缀的固定文本。
pub(super) fn format_prefixed(prefix: &str, entropy: &[u8; RANDOM_BYTES]) -> String {
    let mut text = String::with_capacity(TEXT_LEN);
    text.push_str(prefix);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in entropy {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            text.push(char::from(ALPHABET[((buffer >> bits) & 0x3f) as usize]));
        }
        buffer &= (1_u32 << bits) - 1;
    }
    if bits > 0 {
        text.push(char::from(ALPHABET[((buffer << (6 - bits)) & 0x3f) as usize]));
    }
    text
}

/// 严格解析；不接受 `=`、其他长度、未知前缀或非规范填充位。
pub(super) fn parse_prefixed(prefix: &str, text: &str) -> Result<[u8; RANDOM_BYTES], TextFault> {
    let bytes = text.as_bytes();
    if bytes.contains(&b'=') {
        return Err(TextFault::Alphabet);
    }
    if bytes.len() != TEXT_LEN {
        return Err(TextFault::Length);
    }
    if !bytes.starts_with(prefix.as_bytes()) {
        return Err(TextFault::Prefix);
    }
    let entropy = decode_body(&bytes[PREFIX_LEN..])?;
    if format_prefixed(prefix, &entropy) != text {
        return Err(TextFault::NonCanonical);
    }
    Ok(entropy)
}

fn decode_body(body: &[u8]) -> Result<[u8; RANDOM_BYTES], TextFault> {
    let mut output = [0_u8; RANDOM_BYTES];
    let mut written = 0;
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in body {
        let value = decode_char(*byte).ok_or(TextFault::Alphabet)?;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            if written >= RANDOM_BYTES {
                return Err(TextFault::Length);
            }
            output[written] = ((buffer >> bits) & 0xff) as u8;
            written += 1;
        }
        buffer &= (1_u32 << bits) - 1;
    }
    if bits > 0 && buffer != 0 {
        return Err(TextFault::NonCanonical);
    }
    if written != RANDOM_BYTES {
        return Err(TextFault::Length);
    }
    Ok(output)
}

fn decode_char(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

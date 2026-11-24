//! 预约令牌 `C1-<43 字符无填充 base64url>` 的编码、解析与摘要。
//!
//! 随机量固定 32 字节；领域只做确定性转换。令牌文本与会话秘密不实现 `Debug` 原文输出、
//! 序列化或日志显示，调用方只在本人寄件指引中解封。

use std::collections::HashSet;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::consignment::deposit::errors::DepositError;

/// 预约令牌前缀。
pub const TOKEN_PREFIX: &str = "C1-";
/// 预约令牌正文的固定字符数。
pub const TOKEN_BODY_LEN: usize = 43;
/// 完整令牌文本的 ASCII 字节数。
pub const TOKEN_TEXT_LEN: usize = 46;
/// 随机量字节数。
pub const TOKEN_RANDOM_BYTES: usize = 32;
/// 寄售正文固定件数上限。
pub const DEPOSIT_ITEM_LIMIT: usize = 20;
/// 寄售正文固定原始字节上限。
pub const DEPOSIT_BYTE_LIMIT: usize = 2_048;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 32 字节预约随机量。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ReservationToken([u8; TOKEN_RANDOM_BYTES]);

impl ReservationToken {
    /// 从密码适配器提供的 32 字节随机量构造。
    pub const fn from_entropy(entropy: [u8; TOKEN_RANDOM_BYTES]) -> Self {
        Self(entropy)
    }

    /// 借用原始随机量；只用于摘要或持久化适配器。
    pub const fn as_bytes(&self) -> &[u8; TOKEN_RANDOM_BYTES] {
        &self.0
    }
}

impl fmt::Debug for ReservationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 已规范化的完整预约令牌文本；禁止 `Debug` 原文、序列化与日志输出。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TokenText(String);

impl TokenText {
    /// 借用完整令牌文本；调用方只在本人寄件指引中解封。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TokenText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 预约令牌摘要；固定为规范令牌文本 ASCII 的 SHA-256。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenDigest([u8; 32]);

impl TokenDigest {
    /// 返回固定 32 字节摘要。
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// 把 32 字节随机量编码为固定令牌文本。
pub fn format_token(entropy: [u8; TOKEN_RANDOM_BYTES]) -> TokenText {
    let body = encode_body(&entropy);
    let mut text = String::with_capacity(TOKEN_TEXT_LEN);
    text.push_str(TOKEN_PREFIX);
    for byte in body {
        text.push(char::from(byte));
    }
    TokenText(text)
}

/// 严格解析规范令牌文本；不接受 `=`、其他长度、未知前缀或非规范填充位。
pub fn parse_token(text: &str) -> Result<ReservationToken, DepositError> {
    let bytes = text.as_bytes();
    if bytes.contains(&b'=') {
        return Err(DepositError::TokenAlphabet);
    }
    if bytes.len() != TOKEN_TEXT_LEN {
        return Err(DepositError::TokenLength);
    }
    if !bytes.starts_with(TOKEN_PREFIX.as_bytes()) {
        return Err(DepositError::TokenPrefix);
    }
    let body = &bytes[TOKEN_PREFIX.len()..];
    let entropy = decode_body(body)?;
    // 解码后重新编码逐字比较，拒绝任何非唯一编码。
    let canonical = format_token(entropy);
    if canonical.as_str() != text {
        return Err(DepositError::TokenNonCanonical);
    }
    Ok(ReservationToken(entropy))
}

/// 计算规范令牌文本 ASCII 的 SHA-256。
pub fn token_digest(token: &ReservationToken) -> TokenDigest {
    let text = format_token(token.0);
    let digest = Sha256::digest(text.as_str().as_bytes());
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    TokenDigest(bytes)
}

/// 主题 `consign` 按 ASCII 忽略大小写匹配，不 trim。
pub fn is_consign_subject(subject: &str) -> bool {
    subject.eq_ignore_ascii_case("consign")
}

/// 逐行解析寄售正文，保留原引用顺序。
///
/// 不变量 6、7：超限、自由文本、重复引用与未知版本全部失败关闭，不截断或猜测。
///
/// 有效上限取 `min(20, protocol_item_limit)` 与 `min(2048, protocol_byte_limit)`；超出即失败，
/// 不截断后接受。接受 LF、CRLF 与恰好一个结尾换行；其他空行、孤立 CR 与重复码拒绝。
pub fn parse_deposit_body(
    body: &[u8],
    protocol_item_limit: usize,
    protocol_byte_limit: usize,
) -> Result<Vec<ReservationToken>, DepositError> {
    let item_limit = DEPOSIT_ITEM_LIMIT.min(protocol_item_limit);
    let byte_limit = DEPOSIT_BYTE_LIMIT.min(protocol_byte_limit);
    if body.len() > byte_limit {
        return Err(DepositError::BodyTooLarge);
    }
    if body.is_empty() {
        return Err(DepositError::BodyEmpty);
    }

    let mut data = body;
    if data.last() == Some(&b'\n') {
        data = &data[..data.len() - 1];
        if data.last() == Some(&b'\r') {
            data = &data[..data.len() - 1];
        }
    }
    if data.is_empty() {
        return Err(DepositError::BodyEmpty);
    }

    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in data.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        lines.push(strip_crlf(&data[start..index])?);
        start = index + 1;
    }
    let last = &data[start..];
    if last.contains(&b'\r') {
        return Err(DepositError::BodyIsolatedCr);
    }
    lines.push(last);

    let mut tokens = Vec::new();
    let mut seen = HashSet::new();
    for line in lines {
        if line.is_empty() {
            return Err(DepositError::BodyBlankLine);
        }
        if !line.starts_with(TOKEN_PREFIX.as_bytes()) {
            if line.starts_with(b"c1-") || line.starts_with(b"C2-") {
                return Err(DepositError::TokenPrefix);
            }
            return Err(DepositError::BodyFreeText);
        }
        let text = std::str::from_utf8(line).map_err(|_| DepositError::BodyFreeText)?;
        let token = parse_token(text)?;
        if !seen.insert(*token.as_bytes()) {
            return Err(DepositError::BodyDuplicate);
        }
        tokens.push(token);
        if tokens.len() > item_limit {
            return Err(DepositError::TooManyItems);
        }
    }
    Ok(tokens)
}

fn strip_crlf(line: &[u8]) -> Result<&[u8], DepositError> {
    match line.split_last() {
        Some((b'\r', rest)) if !rest.contains(&b'\r') => Ok(rest),
        Some((_, _)) if line.contains(&b'\r') => Err(DepositError::BodyIsolatedCr),
        _ => Ok(line),
    }
}

fn encode_body(entropy: &[u8; TOKEN_RANDOM_BYTES]) -> [u8; TOKEN_BODY_LEN] {
    let mut output = [b'A'; TOKEN_BODY_LEN];
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    let mut written = 0;
    for byte in entropy {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            output[written] = ALPHABET[((buffer >> bits) & 0x3f) as usize];
            written += 1;
        }
        buffer &= (1_u32 << bits) - 1;
    }
    if bits > 0 {
        output[written] = ALPHABET[((buffer << (6 - bits)) & 0x3f) as usize];
    }
    output
}

fn decode_body(body: &[u8]) -> Result<[u8; TOKEN_RANDOM_BYTES], DepositError> {
    let mut output = [0_u8; TOKEN_RANDOM_BYTES];
    let mut written = 0;
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in body {
        let value = decode_char(*byte).ok_or(DepositError::TokenAlphabet)?;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            if written >= TOKEN_RANDOM_BYTES {
                return Err(DepositError::TokenLength);
            }
            output[written] = ((buffer >> bits) & 0xff) as u8;
            written += 1;
        }
        buffer &= (1_u32 << bits) - 1;
    }
    if bits > 0 && buffer != 0 {
        return Err(DepositError::TokenNonCanonical);
    }
    if written != TOKEN_RANDOM_BYTES {
        return Err(DepositError::TokenLength);
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

#[cfg(test)]
mod tests {
    use super::{
        DEPOSIT_BYTE_LIMIT, DEPOSIT_ITEM_LIMIT, TOKEN_PREFIX, format_token, is_consign_subject,
        parse_deposit_body, parse_token, token_digest,
    };
    use crate::consignment::deposit::errors::DepositError;

    fn token_text(byte: u8) -> String {
        format_token([byte; 32]).as_str().to_owned()
    }

    #[test]
    fn zero_vector_is_c1_plus_43_a_and_round_trips() {
        let text = format_token([0; 32]);
        let expected = format!("C1-{}", "A".repeat(43));
        assert_eq!(text.as_str(), expected);
        assert_eq!(parse_token(&expected).map(|token| *token.as_bytes()), Ok([0; 32]));
    }

    #[test]
    fn arbitrary_entropy_round_trips_and_digest_is_canonical_sha256() {
        let mut entropy = [0_u8; 32];
        for (index, byte) in entropy.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7).wrapping_add(3);
        }
        let text = format_token(entropy);
        let token = parse_token(text.as_str()).expect("规范令牌应可解析");
        assert_eq!(*token.as_bytes(), entropy);
        let digest = token_digest(&token);
        assert_eq!(digest, token_digest(&parse_token(text.as_str()).expect("重解析")));
        assert_ne!(digest, token_digest(&parse_token(&token_text(1)).expect("另一令牌")));
    }

    #[test]
    fn parser_rejects_padding_length_alphabet_prefix_and_noncanonical_tail() {
        let canonical = "A".repeat(43);
        assert_eq!(parse_token(&format!("C1-{canonical}=")), Err(DepositError::TokenAlphabet));
        assert_eq!(parse_token(&format!("C1-{}", "A".repeat(42))), Err(DepositError::TokenLength));
        assert_eq!(
            parse_token(&format!("C1-{}!", "A".repeat(42))),
            Err(DepositError::TokenAlphabet)
        );
        assert_eq!(parse_token(&format!("c1-{canonical}")), Err(DepositError::TokenPrefix));
        assert_eq!(parse_token(&format!("C2-{canonical}")), Err(DepositError::TokenPrefix));
        // 最后一个字符只承载 4 个有效位；值为 1 时最低两位非零。
        assert_eq!(
            parse_token(&format!("C1-{}B", "A".repeat(42))),
            Err(DepositError::TokenNonCanonical)
        );
    }

    #[test]
    fn subject_is_ascii_case_insensitive_without_trimming() {
        for subject in ["consign", "CONSIGN", "CoNsIgN"] {
            assert!(is_consign_subject(subject), "{subject}");
        }
        for subject in [" consign", "consign ", "consign\n", "ｃｏｎｓｉｇｎ", "auction", ""]
        {
            assert!(!is_consign_subject(subject), "{subject:?}");
        }
    }

    #[test]
    fn body_accepts_lf_crlf_and_exactly_one_trailing_newline() {
        let first = token_text(1);
        let second = token_text(2);
        for body in [
            format!("{first}\n{second}"),
            format!("{first}\r\n{second}"),
            format!("{first}\n{second}\n"),
            format!("{first}\r\n{second}\r\n"),
        ] {
            let parsed = parse_deposit_body(body.as_bytes(), 20, 2048).expect("正文应接受");
            assert_eq!(parsed.len(), 2);
            assert_eq!(parsed[0], parse_token(&first).expect("首项"));
            assert_eq!(parsed[1], parse_token(&second).expect("次项"));
        }
    }

    #[test]
    fn body_rejects_blank_lines_isolated_cr_duplicates_and_two_trailing_newlines() {
        let first = token_text(1);
        let second = token_text(2);
        for (body, error) in [
            (format!("\n{first}"), DepositError::BodyBlankLine),
            (format!("{first}\n\n{second}"), DepositError::BodyBlankLine),
            (format!("{first}\n{second}\n\n"), DepositError::BodyBlankLine),
            (format!("{first}\r{second}"), DepositError::BodyIsolatedCr),
            (format!("{first}\r"), DepositError::BodyIsolatedCr),
            (format!("{first}\n{first}"), DepositError::BodyDuplicate),
            (format!("{first}\n{first}\n"), DepositError::BodyDuplicate),
        ] {
            assert_eq!(parse_deposit_body(body.as_bytes(), 20, 2048), Err(error), "{body:?}");
        }
    }

    #[test]
    fn body_rejects_free_text_and_unknown_versions() {
        for body in [
            "hello".to_owned(),
            format!("{TOKEN_PREFIX} {}", "A".repeat(43)),
            format!("c1-{}", "A".repeat(43)),
            format!("C2-{}", "A".repeat(43)),
        ] {
            assert!(
                matches!(
                    parse_deposit_body(body.as_bytes(), 20, 2048),
                    Err(DepositError::BodyFreeText
                        | DepositError::TokenPrefix
                        | DepositError::TokenAlphabet
                        | DepositError::TokenLength)
                ),
                "{body:?}"
            );
        }
    }

    #[test]
    fn body_uses_minimum_of_fixed_and_protocol_limits() {
        let first = token_text(1);
        let second = token_text(2);
        assert_eq!(
            parse_deposit_body(format!("{first}\n{second}").as_bytes(), 1, 2048),
            Err(DepositError::TooManyItems)
        );
        assert_eq!(parse_deposit_body(first.as_bytes(), 20, 45), Err(DepositError::BodyTooLarge));
        assert_eq!(parse_deposit_body(first.as_bytes(), 20, 46).map(|items| items.len()), Ok(1));
        assert_eq!(parse_deposit_body(first.as_bytes(), 20, 0), Err(DepositError::BodyTooLarge));
        assert_eq!(parse_deposit_body(first.as_bytes(), 0, 2048), Err(DepositError::TooManyItems));
    }

    #[test]
    fn twenty_items_accepted_and_twenty_one_rejected() {
        let items = (0..=20_u8).map(token_text).collect::<Vec<_>>();
        assert_eq!(
            parse_deposit_body(items[..20].join("\n").as_bytes(), 20, 2048)
                .map(|parsed| parsed.len()),
            Ok(20)
        );
        assert_eq!(
            parse_deposit_body(items.join("\n").as_bytes(), 20, 2048),
            Err(DepositError::TooManyItems)
        );
    }

    #[test]
    fn raw_byte_boundary_is_exclusive_above_2048() {
        let first = token_text(1);
        let mut exact = format!("{first}\n").into_bytes();
        exact.resize(DEPOSIT_BYTE_LIMIT, b'x');
        assert_ne!(
            parse_deposit_body(&exact, DEPOSIT_ITEM_LIMIT, DEPOSIT_BYTE_LIMIT),
            Err(DepositError::BodyTooLarge)
        );
        let mut over = exact;
        over.push(b'x');
        assert_eq!(
            parse_deposit_body(&over, DEPOSIT_ITEM_LIMIT, DEPOSIT_BYTE_LIMIT),
            Err(DepositError::BodyTooLarge)
        );
    }

    #[test]
    fn empty_and_newline_only_bodies_are_empty() {
        for body in ["", "\n", "\r\n"] {
            assert_eq!(parse_deposit_body(body.as_bytes(), 20, 2048), Err(DepositError::BodyEmpty));
        }
    }
}

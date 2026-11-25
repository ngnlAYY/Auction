//! 挑战码：128 位熵、Crockford base32 文本、尝试与发信限额。
//!
//! 码只在持有人与传输边界出现；库里只存 MAC。`Debug` 打码、不序列化、错误不回显输入。

use std::fmt;

use auction_time::{Duration, Timestamp};

use super::errors::RightsError;

/// 挑战码熵字节数：128 位下限。
pub const CHALLENGE_CODE_ENTROPY_BYTES: usize = 16;
/// 规范文本的 Crockford base32 字符数（130 位容量，首字符只用低 3 位）。
pub const CHALLENGE_CODE_TEXT_LEN: usize = 26;
/// 挑战有效期。
pub const CHALLENGE_TTL: Duration = Duration::minutes(10);
/// 每个挑战最多失败次数；第 5 次失败后挑战作废。
pub const MAX_FAILED_ATTEMPTS: u8 = 5;
/// 每权利或每冻结身份两封通知的最短间隔。
pub const NOTICE_INTERVAL: Duration = Duration::minutes(10);

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 用户输入的最大原始长度（含分组连字符），超出直接判长度无效，避免处理超长输入。
const MAX_INPUT_LEN: usize = 40;

/// 128 位挑战码；`Debug` 打码，不实现序列化。
#[derive(Clone)]
pub struct ChallengeCode([u8; CHALLENGE_CODE_ENTROPY_BYTES]);

impl ChallengeCode {
    /// 从密码适配器提供的随机量构造。
    pub const fn from_entropy(entropy: [u8; CHALLENGE_CODE_ENTROPY_BYTES]) -> Self {
        Self(entropy)
    }

    /// 借用原始随机量；只用于 MAC 与封装。
    pub const fn as_bytes(&self) -> &[u8; CHALLENGE_CODE_ENTROPY_BYTES] {
        &self.0
    }
}

impl fmt::Debug for ChallengeCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 规范挑战码文本；`Debug` 打码。
#[derive(Clone, PartialEq, Eq)]
pub struct ChallengeCodeText(String);

impl ChallengeCodeText {
    /// 借用 26 字符规范文本。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 分组显示文本：5-5-5-5-6，仅供展示；解析接受带或不带连字符。
    pub fn display(&self) -> String {
        let text = &self.0;
        let groups = [&text[0..5], &text[5..10], &text[10..15], &text[15..20], &text[20..]];
        groups.join("-")
    }
}

impl fmt::Debug for ChallengeCodeText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 把 128 位随机量编码为 26 字符规范文本。
pub fn format_challenge_code(code: &ChallengeCode) -> ChallengeCodeText {
    let value = u128::from_be_bytes(code.0);
    let mut text = String::with_capacity(CHALLENGE_CODE_TEXT_LEN);
    for index in 0..CHALLENGE_CODE_TEXT_LEN {
        let shift = 5 * (CHALLENGE_CODE_TEXT_LEN - 1 - index);
        text.push(char::from(ALPHABET[((value >> shift) & 0x1f) as usize]));
    }
    ChallengeCodeText(text)
}

/// 解析用户输入：忽略大小写与分组连字符，按 Crockford 把 `I/L` 视作 1、`O` 视作 0。
pub fn parse_challenge_code(input: &str) -> Result<ChallengeCode, RightsError> {
    if input.len() > MAX_INPUT_LEN {
        return Err(RightsError::CodeLength);
    }
    let mut value = 0_u128;
    let mut count = 0_usize;
    for byte in input.bytes().filter(|byte| *byte != b'-') {
        let digit = decode_char(byte).ok_or(RightsError::CodeAlphabet)?;
        // 首字符只有低 3 位有意义；更高位会被 u128 丢弃，必须拒绝以保证唯一编码。
        if count == 0 && digit > 7 {
            return Err(RightsError::CodeNonCanonical);
        }
        value = (value << 5) | u128::from(digit);
        count += 1;
        if count > CHALLENGE_CODE_TEXT_LEN {
            return Err(RightsError::CodeLength);
        }
    }
    if count != CHALLENGE_CODE_TEXT_LEN {
        return Err(RightsError::CodeLength);
    }
    Ok(ChallengeCode(value.to_be_bytes()))
}

fn decode_char(byte: u8) -> Option<u8> {
    let upper = byte.to_ascii_uppercase();
    let upper = match upper {
        b'I' | b'L' => b'1',
        b'O' => b'0',
        other => other,
    };
    ALPHABET
        .iter()
        .position(|candidate| *candidate == upper)
        .and_then(|index| u8::try_from(index).ok())
}

/// 挑战当前的可验证状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChallengeState {
    /// 截止时刻。
    pub expires_at: Timestamp,
    /// 已失败次数。
    pub failed_attempts: u8,
    /// 是否已被成功核验消费。
    pub consumed: bool,
}

impl ChallengeState {
    /// 只有未过期、未消费且失败次数未用尽的挑战才允许核验。
    ///
    /// 保护的不变量：最多 5 次失败；用尽后即使码正确也拒绝。错误统一为 `ChallengeInvalid`，
    /// 过期、已消费与用尽彼此不可区分。
    pub fn ensure_open(&self, now: Timestamp) -> Result<(), RightsError> {
        if self.consumed || now >= self.expires_at || self.failed_attempts >= MAX_FAILED_ATTEMPTS {
            return Err(RightsError::ChallengeInvalid);
        }
        Ok(())
    }

    /// 记一次失败。
    pub const fn after_failure(self) -> Self {
        Self { failed_attempts: self.failed_attempts.saturating_add(1), ..self }
    }
}

/// 距上一封同权利或同身份通知是否已满 10 分钟；没有上一封时允许。
pub fn notice_allowed(last_sent_at: Option<Timestamp>, now: Timestamp) -> bool {
    match last_sent_at {
        None => true,
        Some(last) => now.signed_duration_since(last) >= NOTICE_INTERVAL,
    }
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{
        CHALLENGE_CODE_ENTROPY_BYTES, CHALLENGE_CODE_TEXT_LEN, CHALLENGE_TTL, ChallengeCode,
        ChallengeState, MAX_FAILED_ATTEMPTS, format_challenge_code, notice_allowed,
        parse_challenge_code,
    };
    use crate::consignment::rights::RightsError;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_timestamp(seconds, 0).expect("时刻")
    }

    #[test]
    fn entropy_is_at_least_128_bits_and_text_round_trips() {
        const { assert!(CHALLENGE_CODE_ENTROPY_BYTES * 8 >= 128) };
        const { assert!(CHALLENGE_CODE_TEXT_LEN * 5 >= 128) };
        let mut entropy = [0_u8; CHALLENGE_CODE_ENTROPY_BYTES];
        for (index, byte) in entropy.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
        }
        let code = ChallengeCode::from_entropy(entropy);
        let text = format_challenge_code(&code);
        assert_eq!(text.as_str().len(), 26);
        let parsed = parse_challenge_code(text.as_str()).expect("规范文本");
        assert_eq!(parsed.as_bytes(), &entropy);
        let shown = text.display();
        assert_eq!(shown.matches('-').count(), 4);
        assert_eq!(
            parse_challenge_code(&shown.to_lowercase()).expect("分组小写").as_bytes(),
            &entropy
        );
        let max = format_challenge_code(&ChallengeCode::from_entropy([0xff; 16]));
        assert!(parse_challenge_code(max.as_str()).is_ok(), "全 1 向量可解析");
    }

    #[test]
    fn parser_rejects_bad_length_alphabet_and_noncanonical_head() {
        let valid = format_challenge_code(&ChallengeCode::from_entropy([1; 16]));
        let text = valid.as_str();
        assert_eq!(parse_challenge_code(&text[..25]).map(|_| ()), Err(RightsError::CodeLength));
        assert_eq!(
            parse_challenge_code(&format!("{text}0")).map(|_| ()),
            Err(RightsError::CodeLength)
        );
        assert_eq!(
            parse_challenge_code(&format!("{}U", &text[..25])).map(|_| ()),
            Err(RightsError::CodeAlphabet)
        );
        assert_eq!(
            parse_challenge_code(&format!("8{}", &text[1..])).map(|_| ()),
            Err(RightsError::CodeNonCanonical)
        );
        assert_eq!(parse_challenge_code(&"0".repeat(80)).map(|_| ()), Err(RightsError::CodeLength));
    }

    /// 安全边界：码不进 Debug 与错误文本。
    #[test]
    fn code_secret_never_appears_in_debug_or_error() {
        let code = ChallengeCode::from_entropy([0x42; 16]);
        let text = format_challenge_code(&code);
        assert!(!format!("{code:?}{text:?}").contains(text.as_str()));
        let error =
            parse_challenge_code(&format!("{}!", &text.as_str()[..25])).expect_err("字符错误");
        assert!(!format!("{error} {error:?}").contains(&text.as_str()[..10]));
        assert!(text.display().contains(&text.as_str()[..5]), "反例对照：显式取文本才可见");
    }

    #[test]
    fn fifth_failure_closes_challenge_even_for_correct_code() {
        let mut state =
            ChallengeState { expires_at: at(1_000), failed_attempts: 0, consumed: false };
        for _ in 0..MAX_FAILED_ATTEMPTS - 1 {
            state = state.after_failure();
            assert_eq!(state.ensure_open(at(10)), Ok(()));
        }
        state = state.after_failure();
        assert_eq!(state.ensure_open(at(10)), Err(RightsError::ChallengeInvalid));
    }

    #[test]
    fn expiry_and_consumption_close_challenge_with_same_error() {
        let issued = at(0);
        let expires_at = issued.checked_add_signed(CHALLENGE_TTL).expect("截止");
        let open = ChallengeState { expires_at, failed_attempts: 0, consumed: false };
        assert_eq!(open.ensure_open(at(599)), Ok(()));
        assert_eq!(open.ensure_open(at(600)), Err(RightsError::ChallengeInvalid));
        let consumed = ChallengeState { consumed: true, ..open };
        assert_eq!(consumed.ensure_open(at(1)), Err(RightsError::ChallengeInvalid));
    }

    #[test]
    fn notice_interval_is_ten_minutes() {
        assert!(notice_allowed(None, at(0)));
        assert!(!notice_allowed(Some(at(0)), at(599)));
        assert!(notice_allowed(Some(at(0)), at(600)));
    }
}

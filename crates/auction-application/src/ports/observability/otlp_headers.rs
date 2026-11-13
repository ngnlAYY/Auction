//! OTLP 导出头列表的语法与安全边界。
//! 配置预检与 exporter 初始化共用这一份解析，名称、长度与控制字符边界不会在两处漂移。
//! 导出头属于敏感配置：错误只给出类别，不携带原文。

use std::{borrow::Cow, collections::BTreeSet};

use crate::ports::core::strict_percent_decode;

const MAX_LIST_BYTES: usize = 8 * 1024;
const MAX_ENTRIES: usize = 32;
const MAX_VALUE_BYTES: usize = 1024;

/// 由 exporter 成帧或 W3C 传播管理的头；允许覆盖会破坏请求或追踪上下文。
const RESERVED_NAMES: [&str; 9] = [
    "baggage",
    "connection",
    "content-encoding",
    "content-length",
    "content-type",
    "host",
    "traceparent",
    "tracestate",
    "transfer-encoding",
];

/// OTLP 导出头被拒绝的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpHeadersError {
    UnsafeList,
    TooManyEntries,
    MissingSeparator,
    InvalidEncoding,
    InvalidEntry,
    ReservedOrDuplicateName,
}

impl OtlpHeadersError {
    /// 可直接展示给运维的原因；不含任何配置原文。
    pub const fn reason(self) -> &'static str {
        match self {
            Self::UnsafeList => "OTLP headers 必须是不超过 8192 字节的安全列表",
            Self::TooManyEntries => "OTLP headers 最多包含 32 项",
            Self::MissingSeparator => "OTLP headers 必须使用 key=value 格式",
            Self::InvalidEncoding => "OTLP headers 百分号编码无效",
            Self::InvalidEntry => "OTLP headers 包含非法或过长的键值",
            Self::ReservedOrDuplicateName => "OTLP headers 含有禁止覆盖或重复的键",
        }
    }
}

/// 解析 `key=value,key=value` 形式的导出头，按原顺序返回解码后的键值对；空串表示没有附加头。
/// 键不区分大小写地去重，返回值保留原始大小写。
pub fn parse_otlp_headers(raw: &str) -> Result<Vec<(String, String)>, OtlpHeadersError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    if raw.len() > MAX_LIST_BYTES || raw.chars().any(char::is_control) {
        return Err(OtlpHeadersError::UnsafeList);
    }
    if raw.split(',').count() > MAX_ENTRIES {
        return Err(OtlpHeadersError::TooManyEntries);
    }
    let mut seen = BTreeSet::new();
    let mut headers = Vec::new();
    for part in raw.split(',') {
        let (name, value) = part.split_once('=').ok_or(OtlpHeadersError::MissingSeparator)?;
        // exporter 发送的是解码后的值，长度与控制字符边界必须在解码之后判定。
        let name = percent_decode(name.trim())?;
        let value = percent_decode(value.trim())?;
        if !is_http_token(&name)
            || value.is_empty()
            || value.len() > MAX_VALUE_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(OtlpHeadersError::InvalidEntry);
        }
        let canonical = name.to_ascii_lowercase();
        if RESERVED_NAMES.contains(&canonical.as_str()) || !seen.insert(canonical) {
            return Err(OtlpHeadersError::ReservedOrDuplicateName);
        }
        headers.push((name, value));
    }
    Ok(headers)
}

fn percent_decode(value: &str) -> Result<String, OtlpHeadersError> {
    strict_percent_decode(value).map(Cow::into_owned).ok_or(OtlpHeadersError::InvalidEncoding)
}

/// RFC 7230 `token`：字母数字与固定的标点集合。
fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{OtlpHeadersError, parse_otlp_headers};

    fn pairs(raw: &str) -> Vec<(String, String)> {
        parse_otlp_headers(raw).expect("valid headers")
    }

    fn owned(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect()
    }

    #[test]
    fn empty_input_means_no_headers() {
        assert!(pairs("").is_empty());
    }

    #[test]
    fn entries_are_trimmed_decoded_and_kept_in_order() {
        assert_eq!(
            pairs("Authorization=Bearer%20abc , x-scope = team%2Cblue,x-plus=a+b"),
            owned(&[("Authorization", "Bearer abc"), ("x-scope", "team,blue"), ("x-plus", "a+b")]),
        );
        // 值里的 `=` 属于值；只有第一个 `=` 分隔键值。
        assert_eq!(pairs("x-token=a=b"), owned(&[("x-token", "a=b")]));
    }

    #[test]
    fn list_size_and_entry_count_are_bounded() {
        let at_limit = format!("x-a={}", "v".repeat(1024));
        assert_eq!(pairs(&at_limit).len(), 1);
        let long_value = format!("x-a={}", "v".repeat(1025));
        assert_eq!(parse_otlp_headers(&long_value), Err(OtlpHeadersError::InvalidEntry));

        let list = |count: usize| {
            (0..count).map(|index| format!("x-{index}=v")).collect::<Vec<_>>().join(",")
        };
        assert_eq!(pairs(&list(32)).len(), 32);
        assert_eq!(parse_otlp_headers(&list(33)), Err(OtlpHeadersError::TooManyEntries));

        let oversized = format!("x-a={}", "v".repeat(8 * 1024));
        assert_eq!(parse_otlp_headers(&oversized), Err(OtlpHeadersError::UnsafeList));
        assert_eq!(parse_otlp_headers("x-a=v\n"), Err(OtlpHeadersError::UnsafeList));
    }

    #[test]
    fn malformed_entries_are_rejected_by_category() {
        let cases = [
            ("x-a", OtlpHeadersError::MissingSeparator),
            ("x-a=1,", OtlpHeadersError::MissingSeparator),
            ("x-a=%zz", OtlpHeadersError::InvalidEncoding),
            ("x-a=%4", OtlpHeadersError::InvalidEncoding),
            // 解码结果必须是 UTF-8。
            ("x-a=%ff", OtlpHeadersError::InvalidEncoding),
            ("=v", OtlpHeadersError::InvalidEntry),
            ("x a=v", OtlpHeadersError::InvalidEntry),
            ("x%3Aa=v", OtlpHeadersError::InvalidEntry),
            ("x-a=", OtlpHeadersError::InvalidEntry),
            ("x-a=  ", OtlpHeadersError::InvalidEntry),
            // 控制字符不能借百分号编码绕过首行的原文检查。
            ("x-a=v%0D%0Ainjected", OtlpHeadersError::InvalidEntry),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_otlp_headers(raw), Err(expected), "raw={raw:?}");
        }
    }

    #[test]
    fn reserved_and_duplicate_names_ignore_case() {
        for raw in ["Content-Type=x", "TRACEPARENT=x", "host=x", "x-a=1,baggage=k"] {
            assert_eq!(
                parse_otlp_headers(raw),
                Err(OtlpHeadersError::ReservedOrDuplicateName),
                "raw={raw:?}"
            );
        }
        assert_eq!(
            parse_otlp_headers("X-Key=1,x-key=2"),
            Err(OtlpHeadersError::ReservedOrDuplicateName)
        );
        // 百分号编码后的保留名同样被拒绝。
        assert_eq!(parse_otlp_headers("%68ost=x"), Err(OtlpHeadersError::ReservedOrDuplicateName));
    }

    #[test]
    fn reasons_never_echo_the_input() {
        let secret = "x-api-key=super-secret-value,broken";
        let error = parse_otlp_headers(secret).expect_err("missing separator");
        assert!(!error.reason().contains("super-secret-value"));
    }
}

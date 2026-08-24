//! 浏览器端共用的金额展示格式。

/// 按旧版 Auction-BF 的规则展示 credit 金额。
///
/// 该函数只负责本地展示，不参与金额解析、校验或任何服务端业务判断。
pub fn format_credit_amount(amount: i64) -> String {
    let sign = if amount < 0 { "-" } else { "" };
    let absolute = amount.unsigned_abs();
    if absolute >= 1_000_000 && absolute.is_multiple_of(10_000) {
        let hundredths = absolute / 10_000;
        let whole = hundredths / 100;
        let fraction = (hundredths % 100).to_string();
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            format!("{sign}{whole}M")
        } else {
            format!("{sign}{whole}.{fraction}M")
        }
    } else if absolute.is_multiple_of(1_000) {
        format!("{sign}{}K", absolute / 1_000)
    } else {
        format!("{sign}{}c", format_thousands(absolute))
    }
}

fn format_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

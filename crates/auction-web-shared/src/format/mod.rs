//! 浏览器端共用的金额与比例展示格式。

/// 使用精确的 K/M/B 单位展示 credit 金额，不对出价金额取近似值。
/// 该函数只负责本地展示，不参与金额解析、校验或任何服务端业务判断。
pub fn format_credit_amount(amount: i64) -> String {
    let sign = if amount < 0 { "-" } else { "" };
    let absolute = amount.unsigned_abs();
    if absolute >= 1_000_000_000 && absolute.is_multiple_of(1_000_000) {
        format_scaled_amount(sign, absolute, 1_000_000_000, "B")
    } else if absolute >= 1_000_000 && absolute.is_multiple_of(1_000) {
        format_scaled_amount(sign, absolute, 1_000_000, "M")
    } else if absolute.is_multiple_of(1_000) {
        format!("{sign}{}K", absolute / 1_000)
    } else {
        format!("{sign}{}c", format_thousands(absolute))
    }
}

/// 返回带千位分隔符的完整整数，单位文案由各 surface 的译文提供。
pub fn format_credit_amount_full(amount: i64) -> String {
    let sign = if amount < 0 { "-" } else { "" };
    format!("{sign}{}", format_thousands(amount.unsigned_abs()))
}

fn format_scaled_amount(sign: &str, amount: u64, scale: u64, suffix: &str) -> String {
    let whole = amount / scale;
    // 小数必须先补足前导零，再删除尾零，否则 1.05M 会错误显示成 1.5M。
    let fraction = format!("{:03}", amount % scale / (scale / 1_000));
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{sign}{whole}{suffix}")
    } else {
        format!("{sign}{whole}.{fraction}{suffix}")
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

/// 将累计纳秒格式化为紧凑的持有总时间；只用于展示，不参与结算。
pub fn format_held_duration(nanos: i64) -> String {
    let seconds = (nanos.max(0) as u64) / 1_000_000_000;
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

/// 将百万分比（ppm）展示为去除尾零的百分比，例如 `12_500` 显示为 `1.25%`；负值使用减号 `−`。
pub fn format_ppm(value: i32, show_positive_sign: bool) -> String {
    let absolute = i64::from(value).abs();
    let whole = absolute / 10_000;
    let fraction = absolute % 10_000;
    let number = if fraction == 0 {
        whole.to_string()
    } else {
        let fraction = format!("{fraction:04}").trim_end_matches('0').to_owned();
        format!("{whole}.{fraction}")
    };
    let sign = if value < 0 {
        "−"
    } else if value > 0 && show_positive_sign {
        "+"
    } else {
        ""
    };
    format!("{sign}{number}%")
}

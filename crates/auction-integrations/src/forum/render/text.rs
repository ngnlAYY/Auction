//! 论坛正文的文本处理：BBCode 转义、类型排序与标签、积分与时间格式。

pub(super) fn category_order(value: &str) -> usize {
    // 排序比较器会反复调用，按 ASCII 忽略大小写借用比较，不为每次比较分配小写副本。
    match value {
        value if value.eq_ignore_ascii_case("one-handed") || value.eq_ignore_ascii_case("one") => 1,
        value if value.eq_ignore_ascii_case("two-handed") || value.eq_ignore_ascii_case("two") => 2,
        value if value.eq_ignore_ascii_case("staff") || value.eq_ignore_ascii_case("sta") => 3,
        value if value.eq_ignore_ascii_case("shield") || value.eq_ignore_ascii_case("shd") => 4,
        value if value.eq_ignore_ascii_case("cloth") || value.eq_ignore_ascii_case("clo") => 5,
        value if value.eq_ignore_ascii_case("light") || value.eq_ignore_ascii_case("lig") => 6,
        value if value.eq_ignore_ascii_case("heavy") || value.eq_ignore_ascii_case("hea") => 7,
        _ => usize::MAX,
    }
}

pub(super) fn category_label(value: &str) -> String {
    match category_order(value) {
        1 => "One-Handed".to_owned(),
        2 => "Two-Handed".to_owned(),
        3 => "Staff".to_owned(),
        4 => "Shield".to_owned(),
        5 => "Cloth".to_owned(),
        6 => "Light Armor".to_owned(),
        7 => "Heavy Armor".to_owned(),
        _ => bbcode_text(value),
    }
}

pub(super) fn bbcode_text(value: &str) -> String {
    let value = value.trim();
    let mut output = String::with_capacity(value.len());
    for word in value.split_whitespace() {
        if !output.is_empty() {
            output.push(' ');
        }
        push_bbcode_segment(&mut output, word);
    }
    output
}

/// 保留原始空格并直接写入目标缓冲，避免颜色标签粘连相邻单词或为每个片段另建副本。
pub(super) fn push_bbcode_segment(output: &mut String, value: &str) {
    for character in value.chars() {
        output.push(match character {
            '[' => '［',
            ']' => '］',
            _ => character,
        });
    }
}

pub(super) fn format_forum_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

pub(super) fn format_credits(value: i64) -> String {
    const THOUSAND: i64 = 1_000;
    const MILLION: i64 = 1_000_000;
    if value >= MILLION && value % THOUSAND == 0 {
        let millions = value / MILLION;
        let thousands = (value % MILLION) / THOUSAND;
        if thousands == 0 {
            return format!("{millions}m");
        }
        let fraction = format!("{thousands:03}").trim_end_matches('0').to_owned();
        return format!("{millions}.{fraction}m");
    }
    if value % THOUSAND == 0 {
        return format!("{}k", value / THOUSAND);
    }
    value.to_string()
}

//! 论坛正文的文本处理：BBCode 转义、类型排序与标签、积分与时间格式。

use std::fmt::{self, Write};

use auction_domain::equipment::taxonomy::EquipmentCategory;

/// 向正文追加格式化片段。`String` 的写入不会失败，参数也只用内建 `Display`（数字、`&str`、
/// `Url` 等），没有需要传播的错误；正文是否超限由调用方随后按字节上限判定。
pub(super) fn push_fmt(out: &mut String, args: fmt::Arguments<'_>) {
    let written = out.write_fmt(args);
    // 只在调试构建断言“String 写入不可失败”这一不变量；运行期不存在需要传播或吞掉的错误。
    debug_assert!(written.is_ok(), "String 写入不可失败");
}

/// 目录分类在领域七大类稳定顺序中的位置；按规范名称或 lot 代码前缀识别，不区分 ASCII 大小写。
/// 排序比较器会反复调用，借用比较，不为每次比较分配小写副本。
fn category_index(value: &str) -> Option<usize> {
    EquipmentCategory::ALL.iter().position(|category| {
        value.eq_ignore_ascii_case(category.as_str())
            || value.eq_ignore_ascii_case(category.lot_code_prefix())
    })
}

/// 目录分组顺序沿用领域七大类的稳定顺序；无法识别的分类排在最后。
pub(super) fn category_order(value: &str) -> usize {
    category_index(value).map_or(usize::MAX, |index| index + 1)
}

/// 目录分组标题：轻甲与重甲沿用论坛已发布的 “Armor” 写法，其余用领域规范名称；未知分类原样转义。
pub(super) fn category_label(value: &str) -> String {
    match category_index(value).and_then(|index| EquipmentCategory::ALL.get(index).copied()) {
        Some(EquipmentCategory::Light) => "Light Armor".to_owned(),
        Some(EquipmentCategory::Heavy) => "Heavy Armor".to_owned(),
        Some(category) => category.as_str().to_owned(),
        None => bbcode_text(value),
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

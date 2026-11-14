//! 论坛装备名标色：Peerless 品质词固定彩虹，其余部分按首条命中规则给前缀或后缀上色。

use auction_application::forum::{
    EquipmentColorFacts, EquipmentColorRule, ForumLotSnapshot, PEERLESS_COLORS,
    equipment_color_decision,
};

use super::text::{bbcode_text, push_bbcode_segment, push_fmt};

/// 按规则给装备名加 BBCode 颜色；Peerless 品质词固定彩虹，规则只作用于其余部分。
pub(super) fn colored_equipment_name(
    lot: &ForumLotSnapshot,
    colors: &[EquipmentColorRule],
) -> String {
    let decision = equipment_color_decision(
        colors,
        EquipmentColorFacts {
            name: &lot.name,
            category: &lot.category,
            equipment_type: &lot.equipment_type,
            quality: &lot.equipment_quality,
            level: lot.equipment_level,
            prefix: &lot.equipment_prefix,
            suffix: &lot.equipment_suffix,
        },
    );
    let peerless = decision.peerless;
    let rest = if peerless {
        lot.name.get(8..).unwrap_or_default().trim_start()
    } else {
        lot.name.as_str()
    };
    let mut out = String::new();
    if peerless {
        for (index, character) in "Peerless".chars().enumerate() {
            push_fmt(
                &mut out,
                format_args!(
                    "[color={}]{}[/color]",
                    PEERLESS_COLORS[index.min(PEERLESS_COLORS.len() - 1)],
                    character
                ),
            );
        }
        out.push(' ');
    }
    match decision.matched {
        Some((rule, matched)) if matched.prefix && matched.suffix => {
            push_fmt(&mut out, format_args!("[color={}]", rule.combined_color));
            push_bbcode_segment(&mut out, rest);
            out.push_str("[/color]");
        }
        Some((rule, matched)) if matched.prefix => {
            write_colored_segment(&mut out, rest, &lot.equipment_prefix, &rule.prefix_color, false);
        }
        Some((rule, matched)) if matched.suffix => {
            write_colored_segment(&mut out, rest, &lot.equipment_suffix, &rule.suffix_color, true);
        }
        _ => out.push_str(&bbcode_text(rest)),
    }
    out
}

fn write_colored_segment(out: &mut String, text: &str, needle: &str, color: &str, last: bool) {
    if needle.is_empty() {
        out.push_str(&bbcode_text(text));
        return;
    }
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let start = if last { lower.rfind(&needle) } else { lower.find(&needle) };
    let Some(mut start) = start else {
        out.push_str(&bbcode_text(text));
        return;
    };
    let end = start + needle.len();
    if last && start >= 3 && lower.get(start - 3..start) == Some("of ") {
        start -= 3;
    }
    push_bbcode_segment(out, &text[..start]);
    push_fmt(out, format_args!("[color={color}]"));
    push_bbcode_segment(out, &text[start..end]);
    out.push_str("[/color]");
    push_bbcode_segment(out, &text[end..]);
}

//! 竞得装备邮件的冻结价格变量校验与英文展示；不重算奖惩政策或修改 CoD。

use std::collections::BTreeMap;

use auction_domain::{SETTLEMENT_PPM, validate_settlement_price_breakdown};

use super::exact_variables;

const LEGACY_VARIABLES: &[&str] = &["amount", "item_name", "reference"];
const PRICED_VARIABLES: &[&str] = &[
    "amount",
    "item_name",
    "reference",
    "hold_ppm",
    "adjustment_ppm",
    "winning_bid",
    "adjusted_price",
    "pool_credit",
];

struct WinnerPricing {
    hold_ppm: i64,
    adjustment_ppm: i64,
    winning_bid: i64,
    adjusted_price: i64,
    pool_credit: i64,
}

/// 只接受完整新变量集或精确旧变量集，拒绝半套价格数据和未知字段。
pub(super) fn valid_winner_variables(variables: &BTreeMap<String, String>) -> bool {
    exact_variables(variables, LEGACY_VARIABLES) || winner_pricing(variables).is_some()
}

fn winner_pricing(variables: &BTreeMap<String, String>) -> Option<WinnerPricing> {
    if !exact_variables(variables, PRICED_VARIABLES) {
        return None;
    }
    let integer = |name: &str| {
        let value = variables.get(name)?;
        let parsed = value.parse::<i64>().ok()?;
        (parsed.to_string() == *value).then_some(parsed)
    };
    let pricing = WinnerPricing {
        hold_ppm: integer("hold_ppm")?,
        adjustment_ppm: integer("adjustment_ppm")?,
        winning_bid: integer("winning_bid")?,
        adjusted_price: integer("adjusted_price")?,
        pool_credit: integer("pool_credit")?,
    };
    let amount = integer("amount")?;
    if !(0..=SETTLEMENT_PPM).contains(&pricing.hold_ppm)
        || validate_settlement_price_breakdown(
            pricing.winning_bid,
            pricing.adjustment_ppm,
            pricing.adjusted_price,
            pricing.pool_credit,
        )
        .is_err()
        || amount <= 0
        || pricing.adjusted_price.checked_sub(pricing.pool_credit) != Some(amount)
    {
        return None;
    }
    Some(pricing)
}

/// 历史已准备邮件没有比例事实，保留原载荷并省略明细；不补写为零或套用现行曲线。
pub(super) fn render_price_breakdown(variables: &BTreeMap<String, String>) -> Option<String> {
    if exact_variables(variables, LEGACY_VARIABLES) {
        return Some(String::new());
    }
    let pricing = winner_pricing(variables)?;
    let adjustment_label = match pricing.adjustment_ppm.cmp(&0) {
        std::cmp::Ordering::Less => "discount",
        std::cmp::Ordering::Equal => "no adjustment",
        std::cmp::Ordering::Greater => "surcharge",
    };
    let mut output = format!(
        "Leading time share: {}\nPrice adjustment: {} ({adjustment_label})\n\n\
         Winning bid: {} Credits\nPrice after adjustment: {} Credits\n",
        format_percent(pricing.hold_ppm, false),
        format_percent(pricing.adjustment_ppm, true),
        format_credits(pricing.winning_bid),
        format_credits(pricing.adjusted_price),
    );
    if pricing.pool_credit > 0 {
        output.push_str(&format!(
            "Additional pool credit: {} Credits\n",
            format_credits(pricing.pool_credit)
        ));
    }
    output.push('\n');
    Some(output)
}

fn format_percent(ppm: i64, signed: bool) -> String {
    // PPM 转百分比最多四位小数，整数运算避免舍入后的比例影响账单解释。
    let absolute = ppm.unsigned_abs();
    let sign = if ppm < 0 {
        "-"
    } else if signed && ppm > 0 {
        "+"
    } else {
        ""
    };
    let whole = absolute / 10_000;
    let fraction = absolute % 10_000;
    if fraction == 0 {
        format!("{sign}{whole}%")
    } else {
        let fraction = format!("{fraction:04}");
        format!("{sign}{whole}.{}%", fraction.trim_end_matches('0'))
    }
}

pub(super) fn format_credits(amount: i64) -> String {
    let digits = amount.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

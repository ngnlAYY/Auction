//! 竞得装备邮件的冻结价格变量校验与英文展示；不重算奖惩政策或修改 CoD。

use std::collections::BTreeMap;

use auction_domain::{
    SETTLEMENT_PPM, SettlementMode, validate_settlement_price_breakdown_for_mode,
};

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
    mode: SettlementMode,
    hold_ppm: i64,
    adjustment_ppm: i64,
    winning_bid: i64,
    adjusted_price: i64,
    pool_credit: i64,
}

/// 接受精确旧三/八变量或显式标记 v2 的九变量，拒绝半套价格数据和未知字段。
pub(super) fn valid_winner_variables(variables: &BTreeMap<String, String>) -> bool {
    exact_variables(variables, LEGACY_VARIABLES) || winner_pricing(variables).is_some()
}

fn winner_pricing(variables: &BTreeMap<String, String>) -> Option<WinnerPricing> {
    let mode = match variables.get("pricing_mode").map(String::as_str) {
        None if exact_variables(variables, PRICED_VARIABLES) => SettlementMode::HoldCurveV1,
        Some("hold_curve_v2")
            if variables.len() == PRICED_VARIABLES.len() + 1
                && PRICED_VARIABLES.iter().all(|key| variables.contains_key(*key)) =>
        {
            SettlementMode::HoldCurveV2
        }
        _ => return None,
    };
    let integer = |name: &str| {
        let value = variables.get(name)?;
        let parsed = value.parse::<i64>().ok()?;
        (parsed.to_string() == *value).then_some(parsed)
    };
    let pricing = WinnerPricing {
        mode,
        hold_ppm: integer("hold_ppm")?,
        adjustment_ppm: integer("adjustment_ppm")?,
        winning_bid: integer("winning_bid")?,
        adjusted_price: integer("adjusted_price")?,
        pool_credit: integer("pool_credit")?,
    };
    let amount = integer("amount")?;
    if !(0..=SETTLEMENT_PPM).contains(&pricing.hold_ppm)
        || validate_settlement_price_breakdown_for_mode(
            pricing.mode,
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
    // 旧模式的标签与字节保持原样；v2 明确区分曲线名义比例和预算缩放后的实际优惠。
    let (adjustment_title, price_title, credit_title) =
        if pricing.mode == SettlementMode::HoldCurveV2 {
            (
                "Curve adjustment (before pool scaling)",
                "Price before pool discounts",
                "Pool-funded buyer discount",
            )
        } else {
            ("Price adjustment", "Price after adjustment", "Additional pool credit")
        };
    let mut output = format!(
        "Leading time share: {}\n{adjustment_title}: {} ({adjustment_label})\n\n\
         Winning bid: {} Credits\n{price_title}: {} Credits\n",
        format_percent(pricing.hold_ppm, false),
        format_percent(pricing.adjustment_ppm, true),
        format_credits(pricing.winning_bid),
        format_credits(pricing.adjusted_price),
    );
    if pricing.pool_credit > 0
        || pricing.mode == SettlementMode::HoldCurveV2 && pricing.adjustment_ppm < 0
    {
        output.push_str(&format!(
            "{credit_title}: {} Credits\n",
            format_credits(pricing.pool_credit)
        ));
    }
    if pricing.mode == SettlementMode::HoldCurveV2 && pricing.adjustment_ppm < 0 {
        output.push_str("Base discount is pool-funded and scaled to available funds.\n");
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

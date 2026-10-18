//! 终态尾注只格式化权威时间与已保存 adjustment_ppm，不重算领先比例或结算曲线。

use std::fmt::Write;

use auction_application::{ForumAuctionSnapshot, ForumLotSnapshot, ForumRenderError};
use chrono::{DateTime, Utc};

pub(super) fn valid_snapshot(snapshot: &ForumAuctionSnapshot) -> bool {
    if !snapshot.settled {
        return snapshot.lots.iter().all(|lot| lot.settlement.is_none());
    }
    let Some(end) = snapshot.scheduled_end_at else { return false };
    if end <= snapshot.scheduled_open_at {
        return false;
    }
    snapshot.lots.iter().all(|lot| match (&lot.settlement, lot.status.as_str()) {
        (Some(settlement), "sold") => {
            lot.current_bidder.is_some()
                && settlement.leader_held_nanos >= 0
                && (-1_000_000..=1_000_000).contains(&settlement.adjustment_ppm)
        }
        (None, "unsold") => lot.current_bidder.is_none(),
        _ => false,
    })
}

pub(super) fn write_annotation(
    body: &mut String,
    lot: &ForumLotSnapshot,
    scheduled_open_at: DateTime<Utc>,
    scheduled_end_at: Option<DateTime<Utc>>,
) -> Result<(), ForumRenderError> {
    let Some(settlement) = &lot.settlement else { return Ok(()) };
    let planned_end = scheduled_end_at.ok_or(ForumRenderError::InvalidSnapshot)?;
    let held = format_held_hours(settlement.leader_held_nanos)?;
    let planned = format_hours(scheduled_open_at, planned_end)?;
    write!(
        body,
        "; winning for {held}h / {planned}h; {}%",
        format_adjustment(settlement.adjustment_ppm)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)
}

fn format_held_hours(duration: i64) -> Result<String, ForumRenderError> {
    let duration = i128::from(duration);
    if duration < 0 {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    let tenths = (duration + 180_000_000_000) / 360_000_000_000;
    Ok(format!("{}.{:01}", tenths / 10, tenths % 10))
}

fn format_hours(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<String, ForumRenderError> {
    let nanos = |time: DateTime<Utc>| {
        i128::from(time.timestamp()) * 1_000_000_000 + i128::from(time.timestamp_subsec_nanos())
    };
    let duration = nanos(end) - nanos(start);
    if duration < 0 {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    // 0.1 小时为 360 秒；只对展示四舍五入，不从舍入结果反推比例，软截止允许 X > Y。
    let tenths = (duration + 180_000_000_000) / 360_000_000_000;
    Ok(format!("{}.{:01}", tenths / 10, tenths % 10))
}

fn format_adjustment(ppm: i64) -> String {
    if ppm == 0 {
        return "0".to_owned();
    }
    let sign = if ppm < 0 { "-" } else { "+" };
    let value = ppm.unsigned_abs();
    let integer = value / 10_000;
    let fraction = value % 10_000;
    if fraction == 0 {
        format!("{sign}{integer}")
    } else {
        let fraction = format!("{fraction:04}");
        format!("{sign}{integer}.{}", fraction.trim_end_matches('0'))
    }
}

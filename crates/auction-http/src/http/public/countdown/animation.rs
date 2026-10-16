//! 数字转盘的离散 CSS 动画；节点数量只取决于进制，不随剩余秒数线性增长。

use std::fmt::{self, Write};

/// 输出日/时/分/秒转盘。正毫秒差向上取整，进位在同一秒边界改变。
pub(super) fn write_running(body: &mut String, remaining_ms: i64) -> fmt::Result {
    let seconds = 1 + (remaining_ms - 1) / 1_000;
    let days = format!("{:02}", seconds / 86_400);
    // 应用已限制到纳秒可表示日期，最大差值不足百万天；任何异常都不生成无界 SVG。
    if days.len() > 6 {
        return Err(fmt::Error);
    }
    write!(
        body,
        "<g class=\"countdown-running\" display=\"none\" style=\"--remaining:{remaining_ms}ms\">"
    )?;
    for index in 0..days.len() {
        let power = (days.len() - index - 1) as u32;
        let unit_seconds = 86_400 * 10_i64.pow(power);
        let x = 58 - days.len() as i64 * 15 / 2 + index as i64 * 15;
        write_reel(body, remaining_ms, unit_seconds, 10, x, 15)?;
    }
    for (unit_seconds, radix, x) in [(3_600, 24, 104), (60, 60, 174), (1, 60, 244)] {
        write_reel(body, remaining_ms, unit_seconds, radix, x, 64)?;
    }
    body.push_str("</g>");
    Ok(())
}

fn write_reel(
    body: &mut String,
    remaining_ms: i64,
    unit_seconds: i64,
    radix: i64,
    x: i64,
    width: i64,
) -> fmt::Result {
    let seconds = 1 + (remaining_ms - 1) / 1_000;
    let initial = (seconds / unit_seconds) % radix;
    let step_ms = unit_seconds * 1_000;
    let duration_ms = step_ms * radix;
    // 上取整产生的不足一秒部分只影响第一次跳变；负 delay 对齐所有转盘的低位余数。
    // 如 01:00 在下一秒变为 00:59，不能提前把分钟减一。
    let rounding_ms = seconds * 1_000 - remaining_ms;
    let first_change_ms = (seconds % unit_seconds + 1) * 1_000 - rounding_ms;
    let delay_ms = first_change_ms - step_ms;
    let distance = radix * 36;
    write!(
        body,
        "<svg x=\"{x}\" y=\"42\" width=\"{width}\" height=\"36\" viewBox=\"0 0 {width} 36\" overflow=\"hidden\"><g class=\"countdown-reel\" style=\"--distance:-{distance}px;animation:countdown-roll {duration_ms}ms steps({radix},end) {delay_ms}ms infinite\">"
    )?;
    let digits = if radix == 10 { 1 } else { 2 };
    // 末尾重复首行，下一周期复位时没有视觉跳回；外层在截止瞬间隐藏整个转盘。
    for step in 0..=radix {
        let value = (initial + radix - step) % radix;
        let y = 27 + step * 36;
        write!(body, "<text x=\"{}\" y=\"{y}\">{value:0digits$}</text>", width / 2)?;
    }
    body.push_str("</g></svg>");
    Ok(())
}

//! 独立论坛图片视图；所有文案来自闭集，唯一插值为已校验的时间和整数。

use std::fmt::{self, Write};

use auction_application::PublicAuctionCountdown;

use super::animation::write_running;

/// URL 文件名与图片标题的闭集，不接受用户提供的 SVG、标题或终态文案。
#[derive(Clone, Copy)]
pub(super) enum TimerKind {
    List,
    Auction,
}

impl TimerKind {
    pub(super) fn from_file(file: &str) -> Option<Self> {
        match file {
            "list.svg" => Some(Self::List),
            "auction.svg" => Some(Self::Auction),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::List => "List Timer",
            Self::Auction => "Auction Timer",
        }
    }

    fn finished(self) -> &'static str {
        match self {
            Self::List => "Closed",
            Self::Auction => "Ended",
        }
    }
}

/// 生成有界 SVG，无脚本、外链字体、网络请求或客户端时钟依赖。
pub(super) fn render(
    kind: TimerKind,
    snapshot: &PublicAuctionCountdown,
) -> Result<String, fmt::Error> {
    let deadline = match kind {
        TimerKind::List => Some(snapshot.intake_closes_at),
        TimerKind::Auction => snapshot.scheduled_end_at,
    };
    let title = kind.title();
    let deadline_text = deadline
        .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Not scheduled".to_owned());
    let mut body = String::with_capacity(16_384);
    write!(
        body,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"320\" height=\"124\" viewBox=\"0 0 320 124\" role=\"img\" aria-labelledby=\"timer-title timer-description\"><title id=\"timer-title\">{title}</title><desc id=\"timer-description\">Deadline: {deadline_text}. Planned time only; individual lots may extend. Refresh to read schedule changes; the auction website remains authoritative.</desc>"
    )?;
    write!(body, "<style>{}</style>", include_str!("style.css"))?;
    body.push_str(
        "<rect class=\"countdown-surface\" x=\"1\" y=\"1\" width=\"318\" height=\"122\" rx=\"8\"/>",
    );
    write!(body, "<text class=\"countdown-title\" x=\"160\" y=\"25\">{title}</text>")?;
    let remaining_ms = deadline.map(|time| {
        let remaining = time.signed_duration_since(snapshot.observed_at);
        if remaining <= chrono::Duration::zero() {
            return 0;
        }
        // 正差先向上取毫秒，避免 1000.5ms 提前变成 1s 或 0.5ms 被误判结束。
        // 不把整个跨度转纳秒，跨数百年的合法日期差也不会溢出。
        remaining.num_milliseconds() + i64::from(remaining.subsec_nanos() % 1_000_000 != 0)
    });
    match remaining_ms {
        Some(remaining_ms) if remaining_ms > 0 => {
            // 无动画支持或减少动态效果时只显示日期，不展示停止更新的剩余秒数。
            body.push_str(
                "<text class=\"countdown-fallback\" x=\"160\" y=\"66\">See deadline below</text>",
            );
            write_running(&mut body, remaining_ms)?;
            write!(
                body,
                "<text class=\"countdown-finished\" display=\"none\" x=\"160\" y=\"66\" style=\"--remaining:{remaining_ms}ms\">{}</text>",
                kind.finished()
            )?;
            for (x, unit) in [(58, "DAYS"), (136, "HOURS"), (206, "MINUTES"), (276, "SECONDS")] {
                write!(body, "<text class=\"countdown-unit\" x=\"{x}\" y=\"89\">{unit}</text>")?;
            }
        }
        Some(_) => write!(
            body,
            "<text class=\"countdown-status\" x=\"160\" y=\"66\">{}</text>",
            kind.finished()
        )?,
        None => body
            .push_str("<text class=\"countdown-status\" x=\"160\" y=\"66\">Not scheduled</text>"),
    }
    write!(
        body,
        "<text class=\"countdown-deadline\" x=\"160\" y=\"111\">{deadline_text}</text></svg>"
    )?;
    if body.len() > 32_768 {
        return Err(fmt::Error);
    }
    Ok(body)
}

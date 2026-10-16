//! 论坛首帖的外部 SVG 倒计时链接；仅公开时间，不读取时钟或请求第三方服务。

use std::fmt::Write;

use auction_application::ForumRenderError;
use chrono::{DateTime, SecondsFormat, Utc};
use url::Url;

use super::format_forum_time;

/// 两个计时器只装饰首帖；缺少计划截止时明确显示未排期，不能用开拍时间推算。
pub(super) fn write_countdowns(
    body: &mut String,
    intake_closes_at: DateTime<Utc>,
    scheduled_end_at: Option<DateTime<Utc>>,
) -> Result<(), ForumRenderError> {
    let list_timer = countdown_url("List Timer", "Closed", intake_closes_at)?;
    write!(body, "[img]{list_timer}[/img]").map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(deadline) = scheduled_end_at {
        let auction_timer = countdown_url("Auction Timer", "Ended", deadline)?;
        write!(body, " [img]{auction_timer}[/img]").map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    // 部分论坛访客关闭图片，服务也可能不可用；始终保留带时区的文字与权威截止说明。
    writeln!(
        body,
        "\n[size=1]List Timer: current-round intake closes at bidding open ({}). Auction Timer: {}. Individual lot deadlines may extend; the public auction page remains authoritative.[/size]",
        format_forum_time(intake_closes_at),
        scheduled_end_at.map(format_forum_time).unwrap_or_else(|| "Not scheduled".to_owned()),
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)
}

fn countdown_url(
    title: &str,
    finish: &str,
    deadline: DateTime<Utc>,
) -> Result<Url, ForumRenderError> {
    if deadline.timestamp_nanos_opt().is_none() {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    // 固定 HTTPS origin，不接受任意外部地址；URL 仅含标题、终态标签和公开 UTC 时间。
    let mut url = Url::parse("https://pure-svg-countdown.skk.moe/timer")
        .map_err(|_| ForumRenderError::InvalidSnapshot)?;
    url.query_pairs_mut()
        .append_pair("title", title)
        .append_pair("finish", finish)
        .append_pair("time", &deadline.to_rfc3339_opts(SecondsFormat::Millis, true));
    // 论坛沿用图片扩展名检查；fragment 不会发送给倒计时服务。
    url.set_fragment(Some(".png"));
    Ok(url)
}

//! 论坛首帖的本站倒计时图片链接；没有公开 origin 时由首帖时间列表兜底，不回退第三方。

use std::fmt::Write;

use auction_application::ForumRenderError;
use auction_domain::AuctionId;
use chrono::{DateTime, Utc};
use url::Url;

/// 两个计时器只装饰首帖；缺少计划截止时不生成第二张图片，不能用开拍时间推算。
pub(super) fn write_countdowns(
    body: &mut String,
    auction_id: AuctionId,
    public_url: Option<&str>,
    intake_closes_at: DateTime<Utc>,
    scheduled_end_at: Option<DateTime<Utc>>,
) -> Result<(), ForumRenderError> {
    if intake_closes_at.timestamp_nanos_opt().is_none()
        || scheduled_end_at.is_some_and(|time| time.timestamp_nanos_opt().is_none())
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    if let Some(public_url) = public_url {
        let list_timer = countdown_url(public_url, auction_id, "list.svg")?;
        write!(body, "[img]{list_timer}[/img]").map_err(|_| ForumRenderError::BodyTooLarge)?;
        if scheduled_end_at.is_some() {
            let auction_timer = countdown_url(public_url, auction_id, "auction.svg")?;
            write!(body, " [img]{auction_timer}[/img]")
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        body.push('\n');
    }
    // 图片下不重复附加说明；绝对时间、公开链接及权威状态提示由首帖信息列表和规则保留。
    Ok(())
}

fn countdown_url(
    public_url: &str,
    auction_id: AuctionId,
    timer: &str,
) -> Result<Url, ForumRenderError> {
    let mut url = Url::parse(public_url).map_err(|_| ForumRenderError::InvalidSnapshot)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    // 只复用已由部署配置解析的公开 origin，不把网站 query、用户信息或时间参数带进图片。
    url.set_path(&format!("/api/v1/auctions/{auction_id}/countdown/{timer}"));
    url.set_query(None);
    // 兼容论坛图片后缀检查；真实格式和 Content-Type 仍为 SVG，fragment 不发送给本站。
    url.set_fragment(Some(".png"));
    Ok(url)
}

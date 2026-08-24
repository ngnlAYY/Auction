use std::fmt::Write;

use auction_application::{
    ForumAuctionSnapshot, ForumLotSnapshot, ForumRenderError, ForumRenderedThread, ForumRenderer,
    forum_world_name,
};
use sha2::{Digest, Sha256};
use url::Url;

const MAX_LOTS: usize = 10_000;
const MAX_TITLE_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 65_531;

/// 将同一数据库快照稳定渲染为旧版论坛拍卖主题。
pub fn render_auction_thread(
    snapshot: ForumAuctionSnapshot,
) -> Result<ForumRenderedThread, ForumRenderError> {
    if snapshot.auction_id.is_nil()
        || snapshot.sequence <= 0
        || snapshot.state_version <= 0
        || snapshot.scheduled_open_at.timestamp_nanos_opt().is_none()
        || snapshot.lots.is_empty()
        || snapshot.lots.len() > MAX_LOTS
        || snapshot.public_url.as_deref().is_some_and(|value| !valid_public_url(value))
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }

    let mut lots = snapshot.lots.clone();
    if lots.iter().any(|lot| !valid_lot(lot)) {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    lots.sort_by(|left, right| {
        category_order(&left.category)
            .cmp(&category_order(&right.category))
            .then_with(|| left.code.cmp(&right.code))
    });

    let digest = Sha256::digest(snapshot.auction_id.as_uuid().as_bytes());
    let title = format!(
        "[Auction] HV#{} {} {:02x}{:02x}{:02x}{:02x}",
        snapshot.sequence,
        forum_world_name(snapshot.world),
        digest[0],
        digest[1],
        digest[2],
        digest[3]
    );
    if title.len() > MAX_TITLE_BYTES || title.chars().count() > 50 {
        return Err(ForumRenderError::InvalidSnapshot);
    }

    let mut body = String::new();
    writeln!(&mut body, "{title}").map_err(|_| ForumRenderError::BodyTooLarge)?;
    writeln!(&mut body, "Auction ID: {}", snapshot.auction_id.as_uuid())
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(public_url) = snapshot.public_url.as_deref() {
        writeln!(&mut body, "Public auction page: {public_url}")
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    writeln!(&mut body, "Opens: {}", snapshot.scheduled_open_at.to_rfc3339())
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(opened_at) = snapshot.opened_at {
        writeln!(&mut body, "Opened: {}", opened_at.to_rfc3339())
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    if let Some(scheduled_end_at) = snapshot.scheduled_end_at {
        writeln!(&mut body, "Nominal end: {}", scheduled_end_at.to_rfc3339())
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    body.push('\n');

    let mut last_category = None::<&str>;
    for lot in &lots {
        if last_category != Some(lot.category.as_str()) {
            if last_category.is_some() {
                body.push('\n');
            }
            body.push_str(&lot.category);
            body.push('\n');
            last_category = Some(&lot.category);
        }
        write!(
            &mut body,
            "[{}] {} (seller: {}) Start: {}",
            lot.code,
            clean_single_line(&lot.name),
            clean_single_line(&lot.seller),
            format_credits(lot.start_price)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        if lot.current_price > lot.start_price {
            write!(&mut body, " Current: {}", format_credits(lot.current_price))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
            if let Some(bidder) = lot.current_bidder.as_deref() {
                write!(&mut body, " by {}", clean_single_line(bidder))
                    .map_err(|_| ForumRenderError::BodyTooLarge)?;
            }
        }
        if lot.status != "live" {
            write!(&mut body, " Status: {}", clean_single_line(&lot.status))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        body.push('\n');
        if body.len() > MAX_BODY_BYTES {
            return Err(ForumRenderError::BodyTooLarge);
        }
    }
    body.push_str(
        "\nAuction Rules\nDefault start: 50k\nForum replies use: LotCode amount, LotCode start, cancel, or notify.\nThe public auction page is the authoritative live bid view.\n",
    );
    if body.len() > MAX_BODY_BYTES {
        return Err(ForumRenderError::BodyTooLarge);
    }
    Ok(ForumRenderedThread { title, body })
}

/// 无状态的论坛主题渲染器，供 Worker 通过窄端口调用。
#[derive(Debug, Clone, Copy, Default)]
pub struct ForumThreadRenderer;

impl ForumRenderer for ForumThreadRenderer {
    fn render(
        &self,
        snapshot: ForumAuctionSnapshot,
    ) -> Result<ForumRenderedThread, ForumRenderError> {
        render_auction_thread(snapshot)
    }
}

fn valid_lot(lot: &ForumLotSnapshot) -> bool {
    valid_code(&lot.code)
        && valid_text(&lot.category, 128)
        && valid_text(&lot.name, 1024)
        && valid_text(&lot.seller, 256)
        && lot.start_price > 0
        && lot.current_price >= lot.start_price
        && valid_text(&lot.status, 64)
        && lot.current_bidder.as_deref().is_none_or(|value| valid_text(value, 256))
        && lot.effective_end_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
}

fn valid_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[..3].iter().all(u8::is_ascii_alphabetic)
        && bytes[3..].iter().all(u8::is_ascii_digit)
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_public_url(value: &str) -> bool {
    if !valid_text(value, 2048) {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

fn category_order(value: &str) -> usize {
    match value.to_ascii_lowercase().as_str() {
        "one-handed" | "one" => 1,
        "two-handed" | "two" => 2,
        "staff" | "sta" => 3,
        "shield" | "shd" => 4,
        "cloth" | "clo" => 5,
        "light" | "lig" => 6,
        "heavy" | "hea" => 7,
        _ => usize::MAX,
    }
}

fn clean_single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_credits(value: i64) -> String {
    const THOUSAND: i64 = 1_000;
    const MILLION: i64 = 1_000_000;
    if value % MILLION == 0 {
        return format!("{}m", value / MILLION);
    }
    if value % THOUSAND == 0 {
        let whole = value / THOUSAND;
        if whole >= THOUSAND {
            return format!("{}.{:01}m", whole / THOUSAND, (whole % THOUSAND) / 100);
        }
        return format!("{whole}k");
    }
    value.to_string()
}

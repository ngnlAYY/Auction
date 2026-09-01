use std::fmt::Write;

use auction_application::{
    ForumAuctionSnapshot, ForumLotSnapshot, ForumRenderError, ForumRenderedThread, ForumRenderer,
    PublicEquipmentGameLink, forum_world_name,
};
use sha2::{Digest, Sha256};
use url::Url;

const MAX_LOTS: usize = 10_000;
const MAX_TITLE_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 65_531;

/// 将同一数据库快照稳定渲染为旧版论坛拍卖主题。
pub fn render_auction_thread(
    snapshot: ForumAuctionSnapshot,
    equipment_links: Vec<PublicEquipmentGameLink>,
) -> Result<ForumRenderedThread, ForumRenderError> {
    if snapshot.auction_id.is_nil()
        || snapshot.sequence <= 0
        || snapshot.state_version <= 0
        || snapshot.scheduled_open_at.timestamp_nanos_opt().is_none()
        || snapshot.lots.is_empty()
        || snapshot.lots.len() > MAX_LOTS
        || snapshot.lots.len() != equipment_links.len()
        || snapshot.public_url.as_deref().is_some_and(|value| !valid_public_url(value))
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }

    if snapshot.lots.iter().any(|lot| !valid_lot(lot)) {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    let mut lots = snapshot.lots.into_iter().zip(equipment_links).collect::<Vec<_>>();
    lots.sort_by(|left, right| {
        category_order(&left.0.category)
            .cmp(&category_order(&right.0.category))
            .then_with(|| left.0.code.cmp(&right.0.code))
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
    writeln!(&mut body, "[center][size=5][b]{title}[/b][/size]")
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(public_url) = snapshot.public_url.as_deref() {
        writeln!(&mut body, "[url={public_url}][b]Open the live auction page[/b][/url]")
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    writeln!(&mut body, "[/center]\n").map_err(|_| ForumRenderError::BodyTooLarge)?;
    writeln!(&mut body, "[b]Auction ID:[/b] {}", snapshot.auction_id.as_uuid())
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    writeln!(
        &mut body,
        "[b]Scheduled opening:[/b] {}",
        format_forum_time(snapshot.scheduled_open_at)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(opened_at) = snapshot.opened_at {
        writeln!(&mut body, "[b]Opened:[/b] {}", format_forum_time(opened_at))
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    if let Some(scheduled_end_at) = snapshot.scheduled_end_at {
        writeln!(&mut body, "[b]Nominal end:[/b] {}", format_forum_time(scheduled_end_at))
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    body.push_str(
        "\n[size=4][b]Auction Rules and Bidding[/b][/size]\n\
         • The public auction page is the authoritative live view for prices, leaders and deadlines.\n\
         • Post one command per line. A normal bid looks like [b]One01 200k[/b].\n\
         • A proxy bid looks like [b]One01 proxy 2m fast[/b] or [b]One01 proxy 2m slow[/b].\n\
         • Lower an active proxy with [b]One01 proxy lower 1.5m[/b].\n\
         • Enable MoogleMail outbid notices with [b]One01 notify mm[/b].\n\
         • Amounts are Credits and must use the 1k grid. Extra words make a command invalid.\n\n",
    );

    let nominal_end = snapshot.scheduled_end_at;
    let mut last_category = None::<String>;
    for (lot, equipment_link) in lots {
        if last_category.as_deref() != Some(lot.category.as_str()) {
            if last_category.is_some() {
                body.push('\n');
            }
            writeln!(&mut body, "[size=4][b]{}[/b][/size]", category_label(&lot.category))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
            last_category = Some(lot.category.clone());
        }
        let equipment_url = equipment_link.into_url();
        write!(
            &mut body,
            "[b][{}][/b] [url={}][b]{}[/b][/url] [i](seller: {})[/i] — Start: {}",
            lot.code,
            equipment_url,
            bbcode_text(&lot.name),
            bbcode_text(&lot.seller),
            format_credits(lot.start_price)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        if lot.current_price > lot.start_price {
            write!(&mut body, " | Current: {}", format_credits(lot.current_price))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
            if let Some(bidder) = lot.current_bidder.as_deref() {
                write!(&mut body, " by {}", bbcode_text(bidder))
                    .map_err(|_| ForumRenderError::BodyTooLarge)?;
            }
        }
        if let Some(effective_end_at) = lot.effective_end_at
            && Some(effective_end_at) != nominal_end
        {
            write!(&mut body, " | Ends: {}", format_forum_time(effective_end_at))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        if lot.status != "live" {
            write!(&mut body, " | Status: {}", bbcode_text(&lot.status))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        body.push('\n');
        if body.len() > MAX_BODY_BYTES {
            return Err(ForumRenderError::BodyTooLarge);
        }
    }
    body.push_str("\n[i]This topic is an external projection. The public auction page remains authoritative.[/i]\n");
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
        equipment_links: Vec<PublicEquipmentGameLink>,
    ) -> Result<ForumRenderedThread, ForumRenderError> {
        render_auction_thread(snapshot, equipment_links)
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
        && lot.equipment_link_target.validate().is_ok()
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
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && !value.contains(['[', ']'])
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

fn category_label(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "one-handed" | "one" => "One-Handed".to_owned(),
        "two-handed" | "two" => "Two-Handed".to_owned(),
        "staff" | "sta" => "Staff".to_owned(),
        "shield" | "shd" => "Shield".to_owned(),
        "cloth" | "clo" => "Cloth".to_owned(),
        "light" | "lig" => "Light Armor".to_owned(),
        "heavy" | "hea" => "Heavy Armor".to_owned(),
        _ => bbcode_text(value),
    }
}

fn bbcode_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").replace('[', "［").replace(']', "］")
}

fn format_forum_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_credits(value: i64) -> String {
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

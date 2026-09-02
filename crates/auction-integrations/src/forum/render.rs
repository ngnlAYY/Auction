use std::fmt::Write;

use auction_application::{
    ForumAuctionSnapshot, ForumCurrentBidSource, ForumLotSnapshot, ForumRenderError,
    ForumRenderedPost, ForumRenderedThread, ForumRenderer, ForumSystemLogEntry,
    PublicEquipmentGameLink, forum_catalog_marker, forum_system_log_marker, forum_world_name,
};
use auction_domain::AuctionId;
use sha2::{Digest, Sha256};
use url::Url;

use crate::clients::ehentai::PRODUCTION_ENDPOINT;

const MAX_LOTS: usize = 10_000;
const MAX_TITLE_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 65_531;

/// 将同一数据库快照稳定渲染为经典论坛拍卖主题。
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
        writeln!(&mut body, "[size=3][url={public_url}][b]Open live auction[/b][/url][/size]")
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    writeln!(
        &mut body,
        "[size=1][color=#666666][b]Scheduled opening:[/b] {}[/color][/size]",
        format_forum_time(snapshot.scheduled_open_at)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(opened_at) = snapshot.opened_at {
        writeln!(
            &mut body,
            "[size=1][color=#666666][b]Opened:[/b] {}[/color][/size]",
            format_forum_time(opened_at)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    if let Some(scheduled_end_at) = snapshot.scheduled_end_at {
        writeln!(
            &mut body,
            "[size=1][color=#666666][b]Scheduled end:[/b] {}[/color][/size]",
            format_forum_time(scheduled_end_at)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    writeln!(&mut body, "[/center]").map_err(|_| ForumRenderError::BodyTooLarge)?;
    body.push_str(
        "\n[size=3][b]Auction Rules and Bidding[/b][/size]\n\
         [list]\n\
         [*]The public auction page is authoritative for prices, leaders and deadlines.\n\
         [*]Post one command per line. A normal bid looks like [b]One01 200k[/b].\n\
         [*]Proxy bidding is available only on the public auction page; forum proxy lines are ignored.\n\
         [*]Enable MoogleMail outbid notices with [b]One01 notify mm[/b].\n\
         [*]Amounts are Credits and must use the 1k grid. Extra words make a command invalid.\n\
         [/list]\n",
    );

    let nominal_end = snapshot.scheduled_end_at;
    let mut last_category = None::<String>;
    for (lot, equipment_link) in lots {
        if last_category.as_deref() != Some(lot.category.as_str()) {
            if last_category.is_some() {
                body.push('\n');
            }
            // 分类标题与下一行的 `[code]` 行首是论坛回读解析契约，样式调整不能改变其纯文本形状。
            writeln!(&mut body, "[size=3][b]{}[/b][/size]", category_label(&lot.category))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
            last_category = Some(lot.category.clone());
        }
        let equipment_url = equipment_link.into_url();
        write!(
            &mut body,
            "[b][{}][/b] [url={}][b]{}[/b][/url]",
            lot.code,
            equipment_url,
            bbcode_text(&lot.name)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        let mut equipment_facts = Vec::with_capacity(3);
        if let Some(level) = lot.equipment_level {
            equipment_facts.push(format!("Lv.{level}"));
        }
        if let Some(initials) = lot.primary_attribute_initials.as_deref() {
            equipment_facts.push(initials.to_owned());
        }
        if let Some(avg_roll_tenths) = lot.avg_roll_tenths {
            equipment_facts.push(format!("Ave: {}", format_roll(avg_roll_tenths)));
        }
        if !equipment_facts.is_empty() {
            write!(&mut body, " ({})", equipment_facts.join(", "))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        // seller/start/current 的纯文本标签由论坛目录回读解析器消费，必须保持稳定。
        write!(
            &mut body,
            " (seller: {}) (start: {})",
            bbcode_text(&lot.seller),
            format_credits(lot.start_price)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        // 首笔合法竞价可以恰好等于起拍价；只要已有领先者，也必须展示当前事实与来源。
        if lot.current_price > lot.start_price || lot.current_bidder.is_some() {
            write!(&mut body, " | [b]Current: {}[/b]", format_credits(lot.current_price))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
            if let Some(bidder) = lot.current_bidder.as_deref() {
                write!(&mut body, " by {}", bbcode_text(bidder))
                    .map_err(|_| ForumRenderError::BodyTooLarge)?;
                match lot.current_bid_source.as_ref() {
                    Some(ForumCurrentBidSource::Forum { post_number, thread_id, post_id }) => {
                        write!(
                            &mut body,
                            "（[url={PRODUCTION_ENDPOINT}/index.php?showtopic={thread_id}&p={post_id}#entry{post_id}]#{post_number}[/url]）"
                        )
                        .map_err(|_| ForumRenderError::BodyTooLarge)?;
                    }
                    Some(ForumCurrentBidSource::Proxy) => body.push_str("（代理）"),
                    None => {}
                }
            }
        }
        if let Some(effective_end_at) = lot.effective_end_at
            && Some(effective_end_at) != nominal_end
        {
            write!(&mut body, " | Ends: {}", format_forum_time(effective_end_at))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        if lot.status != "live" {
            write!(&mut body, " | [b]Status:[/b] {}", bbcode_text(&lot.status))
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        body.push('\n');
        if body.len() > MAX_BODY_BYTES {
            return Err(ForumRenderError::BodyTooLarge);
        }
    }
    body.push_str(
        "\n[size=1][color=#666666][i]The public auction page remains authoritative for live auction state.[/i][/color][/size]\n",
    );
    writeln!(
        &mut body,
        "[size=1][color=transparent]{}[/color][/size]",
        forum_catalog_marker(snapshot.auction_id)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
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

    fn render_system_log(
        &self,
        auction_id: AuctionId,
        processed_through_post_number: usize,
        entries: Vec<ForumSystemLogEntry>,
    ) -> Result<ForumRenderedPost, ForumRenderError> {
        render_system_log(auction_id, processed_through_post_number, entries)
    }
}

/// 稳定渲染固定 #2；相同的命令事实始终得到相同正文，避免无意义远端编辑。
pub fn render_system_log(
    auction_id: AuctionId,
    processed_through_post_number: usize,
    mut entries: Vec<ForumSystemLogEntry>,
) -> Result<ForumRenderedPost, ForumRenderError> {
    if auction_id.is_nil() || processed_through_post_number < 2 || entries.len() > 20_000 {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    entries.sort_by(|left, right| {
        left.post_number
            .cmp(&right.post_number)
            .then_with(|| left.post_id.cmp(&right.post_id))
            .then_with(|| left.line_number.cmp(&right.line_number))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
            .then_with(|| left.ignored.cmp(&right.ignored))
    });
    entries.dedup();
    if entries.iter().any(|entry| !valid_system_log_entry(entry)) {
        return Err(ForumRenderError::InvalidSnapshot);
    }

    let warning_count = entries.iter().filter(|entry| !entry.ignored).count();
    let ignored_count = entries.iter().filter(|entry| entry.ignored).count();
    let warning_label = if warning_count == 1 { "warning" } else { "warnings" };
    let ignored_label = if ignored_count == 1 { "post" } else { "posts" };

    let mut body = String::new();
    writeln!(
        &mut body,
        "Updated through [b][size=5]#{processed_through_post_number}[/size][/b] — \
         [b]{warning_count}[/b] {warning_label}, [b]{ignored_count}[/b] manually ignored {ignored_label}."
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    body.push_str(
        "[size=1][color=#666666]Accepted bids and current prices appear in post #1. This fixed post records only edited or invalid commands; bids begin at post #3.[/color][/size]\n\n\
         [center][b][color=#B00020][size=3]Warning Log[/size][/color][/b][/center]\n\n",
    );
    for entry in entries.iter().filter(|entry| !entry.ignored) {
        write_system_log_entry(&mut body, entry)?;
    }
    if warning_count == 0 {
        body.push_str("[color=#666666]No warnings.[/color]\n");
    }
    if ignored_count > 0 {
        body.push_str("\n[center][b][size=3]Ignored Posts[/size][/b][/center]\n\n");
        for entry in entries.iter().filter(|entry| entry.ignored) {
            write_system_log_entry(&mut body, entry)?;
        }
    }
    body.push_str(
        "\n[size=1][color=#666666][i]Maintained by the auction worker. Do not place bids in this post.[/i][/color][/size]\n",
    );
    writeln!(
        &mut body,
        "[size=1][color=transparent]{}[/color][/size]",
        forum_system_log_marker(auction_id)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ForumRenderError::BodyTooLarge);
    }
    Ok(ForumRenderedPost { body })
}

fn write_system_log_entry(
    body: &mut String,
    entry: &ForumSystemLogEntry,
) -> Result<(), ForumRenderError> {
    write!(
        body,
        "[url={PRODUCTION_ENDPOINT}/index.php?act=findpost&pid={}][b]#{}[/b][/url] \
         [color=#777777]",
        entry.post_id, entry.post_number
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(line_number) = entry.line_number {
        write!(body, "line {line_number} · ").map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    writeln!(body, "{}[/color] — {}", bbcode_text(&entry.code), bbcode_text(&entry.message))
        .map_err(|_| ForumRenderError::BodyTooLarge)
}

fn valid_system_log_entry(entry: &ForumSystemLogEntry) -> bool {
    (entry.post_number == 1 || entry.post_number >= 3)
        && valid_forum_post_id(&entry.post_id)
        && entry.line_number.is_none_or(|line| line > 0)
        && valid_text(&entry.code, 128)
        && valid_text(&entry.message, 512)
}

fn valid_forum_post_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

fn valid_lot(lot: &ForumLotSnapshot) -> bool {
    valid_code(&lot.code)
        && valid_text(&lot.category, 128)
        && valid_text(&lot.name, 1024)
        && lot.equipment_level.is_none_or(|level| (1..=100_000).contains(&level))
        && lot.primary_attribute_initials.as_deref().is_none_or(valid_primary_attribute_initials)
        && lot.avg_roll_tenths.is_none_or(|value| (0..=1_000).contains(&value))
        && valid_text(&lot.seller, 256)
        && lot.start_price > 0
        && lot.current_price >= lot.start_price
        && valid_text(&lot.status, 64)
        && lot.current_bidder.as_deref().is_none_or(|value| valid_text(value, 256))
        && lot.current_bid_source.as_ref().is_none_or(valid_current_bid_source)
        && (lot.current_bid_source.is_none() || lot.current_bidder.is_some())
        && lot.effective_end_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
        && lot.equipment_link_target.validate().is_ok()
}

fn valid_current_bid_source(source: &ForumCurrentBidSource) -> bool {
    match source {
        ForumCurrentBidSource::Forum { post_number, thread_id, post_id } => {
            *post_number >= 3 && valid_forum_post_id(thread_id) && valid_forum_post_id(post_id)
        }
        ForumCurrentBidSource::Proxy => true,
    }
}

fn valid_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[..3].iter().all(u8::is_ascii_alphabetic)
        && bytes[3..].iter().all(u8::is_ascii_digit)
}

fn valid_primary_attribute_initials(value: &str) -> bool {
    !value.is_empty() && value.len() <= 6 && value.bytes().all(|byte| byte.is_ascii_uppercase())
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

fn format_roll(tenths: i64) -> String {
    format!("{}.{:01}%", tenths / 10, tenths % 10)
}

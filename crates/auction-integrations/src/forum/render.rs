//! 论坛目录分页与 BBCode 渲染；价格及资格仍由权威拍卖快照决定。

use std::fmt::Write;

#[path = "render/countdown.rs"]
mod countdown;
#[path = "render/log.rs"]
mod log;
#[path = "render/settlement.rs"]
mod settlement;

use auction_application::{
    FORUM_CATALOG_PAGE_SIZE, ForumAuctionSnapshot, ForumCurrentBidSource, ForumLotSnapshot,
    ForumRenderError, ForumRenderedPost, ForumRenderedThread, ForumRenderer, ForumSystemLogEntry,
    MAX_FORUM_CATALOG_LOTS, PublicEquipmentGameLink, forum_catalog_page_marker, forum_world_name,
};
use auction_domain::{AuctionId, SettlementWorld};
use sha2::{Digest, Sha256};
use url::Url;

use crate::clients::ehentai::PRODUCTION_ENDPOINT;

const MAX_TITLE_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 65_531;
const FORUM_SEPARATOR: &str =
    "=========================================================================================";

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
        || snapshot.lots.len() > MAX_FORUM_CATALOG_LOTS
        || snapshot.lots.len() != equipment_links.len()
        || snapshot.public_url.as_deref().is_some_and(|value| !valid_public_url(value))
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }

    if snapshot.lots.iter().any(|lot| !valid_lot(lot)) || !settlement::valid_snapshot(&snapshot) {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    let mut lots = snapshot.lots.into_iter().zip(equipment_links).collect::<Vec<_>>();
    lots.sort_by(|left, right| {
        category_order(&left.0.category)
            .cmp(&category_order(&right.0.category))
            .then_with(|| left.0.code.cmp(&right.0.code))
    });

    let world_title = match snapshot.world {
        SettlementWorld::Main => "Main",
        SettlementWorld::Isekai => "Isekai",
    };
    let digest = Sha256::digest(snapshot.auction_id.as_uuid().as_bytes());
    let legacy_title = format!(
        "[Auction] HV#{} {} {:02x}{:02x}{:02x}{:02x}",
        snapshot.sequence,
        forum_world_name(snapshot.world),
        digest[0],
        digest[1],
        digest[2],
        digest[3]
    );
    let title =
        format!("[Auction] {world_title} Equipment Auction - Session {:03}", snapshot.sequence);
    let description = snapshot.scheduled_end_at.map_or_else(
        || {
            format!(
                "Bid Start:{} / Bid End:Not scheduled",
                format_forum_time(snapshot.scheduled_open_at)
            )
        },
        |scheduled_end_at| {
            format!(
                "Bid Start:{} / Bid End:{}",
                format_forum_time(snapshot.scheduled_open_at),
                format_forum_time(scheduled_end_at)
            )
        },
    );
    if title.len() > MAX_TITLE_BYTES
        || title.chars().count() > 50
        || legacy_title.len() > MAX_TITLE_BYTES
        || description.len() > MAX_TITLE_BYTES
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }

    let mut body = String::new();
    // 本场收件在正式开拍时切换至后继草稿；已有实际开拍时间时优先展示事实时间。
    countdown::write_countdowns(
        &mut body,
        snapshot.auction_id,
        snapshot.public_url.as_deref(),
        snapshot.opened_at.unwrap_or(snapshot.scheduled_open_at),
        snapshot.scheduled_end_at,
    )?;
    // 采用参考主题的左对齐标题、分隔线、嵌套列表和代码块；拍品行仍保持纯文本形状，供目录回读解析。
    writeln!(
        &mut body,
        "[size=5][b][HV Auction System] {world_title} World Equipment Auction - Session {:03}[/b][/size]",
        snapshot.sequence
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    // 论坛回读会删除开列表前、外层闭列表后的换行；直接生成其稳定形状，避免放宽正文比较。
    body.push_str(FORUM_SEPARATOR);
    body.push_str("[list]\n[*][b]World:[/b] ");
    body.push_str(forum_world_name(snapshot.world));
    body.push('\n');
    writeln!(
        &mut body,
        "[*][b]Bidding opens:[/b] {}",
        format_forum_time(snapshot.scheduled_open_at)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if let Some(opened_at) = snapshot.opened_at {
        writeln!(&mut body, "[*][b]Opened:[/b] {}", format_forum_time(opened_at))
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    if let Some(scheduled_end_at) = snapshot.scheduled_end_at {
        writeln!(&mut body, "[*][b]Bidding deadline:[/b] {}", format_forum_time(scheduled_end_at))
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    if let Some(public_url) = snapshot.public_url.as_deref() {
        // 论坛会截短纯 URL 链接文字并把省略号写回 BBCode；内层粗体保留完整地址及回读证明。
        writeln!(
            &mut body,
            "[*][b]Auction address:[/b] [url={public_url}][b]{}[/b][/url]",
            bbcode_text(public_url)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    let system_url = format!("{PRODUCTION_ENDPOINT}/index.php?showtopic=301946");
    writeln!(&mut body, "[*][b]About system:[/b] [url={system_url}]{system_url}[/url]")
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    body.push_str(
        "[/list]\
         [size=3][b]Auction Rules and Bidding[/b][/size]\
         [list]\n\
         [*]The public auction page is authoritative for prices, leaders and deadlines.\n\
         [*][b]Bidding commands:[/b]\
         [list]\n\
         [*]Post one command per line.\n\
         [*]Example: [code]One01 200k[/code]\n\
         [*]Proxy bidding is available only on the public auction page; forum proxy lines are ignored.\n\
         [*]Watch equipment on the public auction page for outbid notices. Choose MoogleMail (default) or forum PM in My Account / Outbid notifications. Forum PM is only for outbid notices, not settlement or COD. New proxy authorizations watch automatically.\n\
         [*]This forum command requests a one-time auction notice, not an outbid subscription. The notify pm forum command remains unavailable:\n\
         [code]One01 notify mm[/code]\n\
         [*]Amounts are Credits and must use the 1k grid. Extra words make a command invalid.\n\
         [/list]\n\
         [/list]",
    );
    writeln!(&mut body, "{FORUM_SEPARATOR}").map_err(|_| ForumRenderError::BodyTooLarge)?;

    let page_size = snapshot.catalog_layout.page_size();
    let page_count = lots.len().div_ceil(page_size);
    if page_count > 1 {
        write_catalog_navigation(&mut body, page_count)?;
    }
    let mut pages = Vec::with_capacity(page_count);
    let nominal_end = snapshot.scheduled_end_at;
    let mut last_category = None::<String>;
    for (index, (lot, equipment_link)) in lots.into_iter().enumerate() {
        if index > 0 && index % page_size == 0 {
            finish_catalog_page(&mut body, snapshot.auction_id, pages.len() + 1)?;
            pages.push(ForumRenderedPost { body: std::mem::take(&mut body) });
            writeln!(
                &mut body,
                "[size=3][b]{world_title} Equipment Auction - Session {:03} - Catalog {}/{}[/b][/size]",
                snapshot.sequence, pages.len() + 1, page_count
            )
            .map_err(|_| ForumRenderError::BodyTooLarge)?;
            write_catalog_navigation(&mut body, page_count)?;
            // 跨帖重复分类标题；不改变分类编号，也不把续页当成上一页的裸文本尾部。
            last_category = None;
        }
        if last_category.as_deref() != Some(lot.category.as_str()) {
            if last_category.is_some() {
                // 分类之间保留两行空白，避免长目录中标题和上一类最后一件装备粘连。
                body.push_str("\n\n");
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
                            " ([url={PRODUCTION_ENDPOINT}/index.php?showtopic={thread_id}&p={post_id}#entry{post_id}]#{post_number}[/url])"
                        )
                        .map_err(|_| ForumRenderError::BodyTooLarge)?;
                    }
                    Some(ForumCurrentBidSource::Proxy) => body.push_str(" (proxy)"),
                    None => {}
                }
            }
        }
        settlement::write_annotation(
            &mut body,
            &lot,
            snapshot.scheduled_open_at,
            snapshot.scheduled_end_at,
        )?;
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
    finish_catalog_page(&mut body, snapshot.auction_id, pages.len() + 1)?;
    pages.push(ForumRenderedPost { body });
    let mut pages = pages.into_iter();
    let first = pages.next().ok_or(ForumRenderError::InvalidSnapshot)?;
    Ok(ForumRenderedThread {
        title,
        description,
        legacy_title: Some(legacy_title),
        body: first.body,
        catalog_continuation: pages.collect(),
    })
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
        catalog_page_count: usize,
        processed_through_post_number: usize,
        entries: Vec<ForumSystemLogEntry>,
    ) -> Result<ForumRenderedPost, ForumRenderError> {
        log::render_system_log(
            auction_id,
            catalog_page_count,
            processed_through_post_number,
            entries,
        )
    }
}

/// 首帖与续页使用同一页尾和独立字节上限，任何一页超限都在远端写入前失败。
fn finish_catalog_page(
    body: &mut String,
    auction_id: AuctionId,
    post_number: usize,
) -> Result<(), ForumRenderError> {
    writeln!(body, "{FORUM_SEPARATOR}").map_err(|_| ForumRenderError::BodyTooLarge)?;
    body.push_str(
        "[size=1][color=#666666][i]The public auction page remains authoritative for live auction state.[/i][/color][/size]\n",
    );
    writeln!(
        body,
        "[size=1][color=transparent]{}[/color][/size]",
        forum_catalog_page_marker(auction_id, post_number)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ForumRenderError::BodyTooLarge);
    }
    Ok(())
}

fn write_catalog_navigation(body: &mut String, page_count: usize) -> Result<(), ForumRenderError> {
    writeln!(
        body,
        "[size=1]Equipment: posts #1-#{page_count} (at most {FORUM_CATALOG_PAGE_SIZE} per post). Updates: #{}. Bids: #{} onward.[/size]\n",
        page_count + 1, page_count + 2
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)
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

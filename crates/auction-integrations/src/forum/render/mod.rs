//! 论坛目录分页与 BBCode 渲染；价格及资格仍由权威拍卖快照决定。

use std::fmt::Write;

mod countdown;
mod log;
mod settlement;
mod text;
mod validation;

use auction_application::forum::{
    EquipmentColorFacts, EquipmentColorRule, FORUM_CATALOG_PAGE_SIZE, ForumAuctionSnapshot,
    ForumCurrentBidSource, ForumLotSnapshot, ForumRenderError, ForumRenderedPost,
    ForumRenderedThread, ForumRenderer, ForumSystemLogEntry, MAX_FORUM_CATALOG_LOTS,
    MAX_FORUM_RECONCILIATION_TITLES, PEERLESS_COLORS, equipment_color_decision,
    forum_catalog_page_marker,
};
use auction_application::public::PublicEquipmentGameLink;
use auction_domain::{AuctionId, AuctionStatus, SettlementWorld, valid_text};
use sha2::{Digest, Sha256};

use crate::clients::ehentai::{MAXIMUM_FORUM_TITLE_RUNES, PRODUCTION_ENDPOINT};
use text::{
    bbcode_text, category_label, category_order, format_credits, format_forum_time,
    push_bbcode_segment,
};
use validation::{valid_forum_post_id, valid_lot, valid_public_url};

const MAX_TITLE_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 65_531;
const FORUM_SEPARATOR: &str =
    "=========================================================================================";

/// 将同一数据库快照按已冻结标色规则稳定渲染为经典论坛拍卖主题。
fn render_auction_thread_with_colors(
    snapshot: ForumAuctionSnapshot,
    equipment_links: Vec<PublicEquipmentGameLink>,
    colors: &[EquipmentColorRule],
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
            .then_with(|| left.0.code.len().cmp(&right.0.code.len()))
            .then_with(|| left.0.code.cmp(&right.0.code))
    });

    let world_title = match snapshot.world {
        SettlementWorld::Main => "Main",
        SettlementWorld::Isekai => "Isekai",
    };
    let title = topic_title(world_title, snapshot.sequence, snapshot.state);
    let mut reconciliation_titles = Vec::with_capacity(MAX_FORUM_RECONCILIATION_TITLES);
    for state in [
        AuctionStatus::Draft,
        AuctionStatus::Preview,
        AuctionStatus::Live,
        AuctionStatus::Final,
        AuctionStatus::Settling,
        AuctionStatus::Settled,
        AuctionStatus::Cancelled,
    ] {
        if state != snapshot.state {
            reconciliation_titles.push(topic_title(world_title, snapshot.sequence, state));
        }
        // 无前缀的上一版状态标题只保留当时 50 字符边界内的实际可发布形式。
        let previous_title = format!(
            "{world_title} Equipment Auction - Session {:03} ({})",
            snapshot.sequence,
            topic_state(state)
        );
        if previous_title.chars().count() <= 50 {
            reconciliation_titles.push(previous_title);
        }
        // 升级前的短标题仍是只读核对候选，不能因改名把未知发帖当成不存在。
        reconciliation_titles.push(format!(
            "[Auction] {world_title} #{:03} ({})",
            snapshot.sequence,
            topic_state(state)
        ));
    }
    let sequence_width = (snapshot.sequence.ilog10() as usize + 1).max(3);
    let previous_title_width = "[Auction] ".len()
        + world_title.len()
        + " Equipment Auction - Session ".len()
        + sequence_width;
    // 更早的无状态标题沿用当时 50 字符边界；不补造或截断历史身份。
    if previous_title_width <= 50 {
        reconciliation_titles.push(format!(
            "[Auction] {world_title} Equipment Auction - Session {:03}",
            snapshot.sequence
        ));
    }
    let digest = Sha256::digest(snapshot.auction_id.as_uuid().as_bytes());
    reconciliation_titles.push(format!(
        "[Auction] HV#{} {} {:02x}{:02x}{:02x}{:02x}",
        snapshot.sequence,
        snapshot.world.as_str(),
        digest[0],
        digest[1],
        digest[2],
        digest[3]
    ));
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
        || title.chars().count() > MAXIMUM_FORUM_TITLE_RUNES
        || reconciliation_titles
            .iter()
            .any(|candidate| candidate.chars().count() > MAXIMUM_FORUM_TITLE_RUNES)
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
    body.push_str(snapshot.world.as_str());
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
            colored_equipment_name(&lot, colors)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        // 装备事实直接追加到最终帖子，避免每件装备分配字段 Vec、临时字符串和 join 副本。
        if lot.equipment_level.is_some()
            || lot.primary_attribute_initials.is_some()
            || lot.avg_roll_tenths.is_some()
        {
            body.push_str(" (");
            let mut separator = "";
            if let Some(level) = lot.equipment_level {
                write!(&mut body, "Lv.{level}").map_err(|_| ForumRenderError::BodyTooLarge)?;
                separator = ", ";
            }
            if let Some(initials) = lot.primary_attribute_initials.as_deref() {
                body.push_str(separator);
                body.push_str(initials);
                separator = ", ";
            }
            if let Some(tenths) = lot.avg_roll_tenths {
                write!(&mut body, "{separator}Ave: {}.{:01}%", tenths / 10, tenths % 10)
                    .map_err(|_| ForumRenderError::BodyTooLarge)?;
            }
            body.push(')');
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
        reconciliation_titles,
        body: first.body,
        catalog_continuation: pages.collect(),
    })
}

fn topic_title(world: &str, sequence: i64, state: AuctionStatus) -> String {
    format!("[Auction] {world} Equipment Auction - Session {sequence:03} ({})", topic_state(state))
}

fn topic_state(state: AuctionStatus) -> &'static str {
    match state {
        AuctionStatus::Draft => "Draft",
        AuctionStatus::Preview => "Preview",
        AuctionStatus::Live => "Live",
        AuctionStatus::Final => "Final",
        AuctionStatus::Settling => "Settling",
        AuctionStatus::Settled => "Settled",
        AuctionStatus::Cancelled => "Cancelled",
    }
}

/// 论坛主题渲染器；颜色规则来自 Admin runtime，渲染器本身无状态。
#[derive(Debug, Clone, Default)]
pub struct ForumThreadRenderer {
    colors: Vec<EquipmentColorRule>,
}

impl ForumThreadRenderer {
    /// 使用启动时已校验的装备标色规则构造渲染器；保留多选维度与规则优先级。
    pub fn with_colors(colors: Vec<EquipmentColorRule>) -> Self {
        Self { colors }
    }
}

impl ForumRenderer for ForumThreadRenderer {
    fn render(
        &self,
        snapshot: ForumAuctionSnapshot,
        equipment_links: Vec<PublicEquipmentGameLink>,
    ) -> Result<ForumRenderedThread, ForumRenderError> {
        render_auction_thread_with_colors(snapshot, equipment_links, &self.colors)
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

/// 按规则给装备名加 BBCode 颜色；Peerless 品质词固定彩虹，规则只作用于其余部分。
fn colored_equipment_name(lot: &ForumLotSnapshot, colors: &[EquipmentColorRule]) -> String {
    let decision = equipment_color_decision(
        colors,
        EquipmentColorFacts {
            name: &lot.name,
            category: &lot.category,
            equipment_type: &lot.equipment_type,
            quality: &lot.equipment_quality,
            level: lot.equipment_level,
            prefix: &lot.equipment_prefix,
            suffix: &lot.equipment_suffix,
        },
    );
    let peerless = decision.peerless;
    let rest = if peerless {
        lot.name.get(8..).unwrap_or_default().trim_start()
    } else {
        lot.name.as_str()
    };
    let mut out = String::new();
    if peerless {
        for (index, character) in "Peerless".chars().enumerate() {
            let _ = write!(
                out,
                "[color={}]{}[/color]",
                PEERLESS_COLORS[index.min(PEERLESS_COLORS.len() - 1)],
                character
            );
        }
        out.push(' ');
    }
    match decision.matched {
        Some((rule, matched)) if matched.prefix && matched.suffix => {
            let _ = write!(out, "[color={}]", rule.combined_color);
            push_bbcode_segment(&mut out, rest);
            out.push_str("[/color]");
        }
        Some((rule, matched)) if matched.prefix => {
            write_colored_segment(&mut out, rest, &lot.equipment_prefix, &rule.prefix_color, false);
        }
        Some((rule, matched)) if matched.suffix => {
            write_colored_segment(&mut out, rest, &lot.equipment_suffix, &rule.suffix_color, true);
        }
        _ => out.push_str(&bbcode_text(rest)),
    }
    out
}

fn write_colored_segment(out: &mut String, text: &str, needle: &str, color: &str, last: bool) {
    if needle.is_empty() {
        out.push_str(&bbcode_text(text));
        return;
    }
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let start = if last { lower.rfind(&needle) } else { lower.find(&needle) };
    let Some(mut start) = start else {
        out.push_str(&bbcode_text(text));
        return;
    };
    let end = start + needle.len();
    if last && start >= 3 && lower.get(start - 3..start) == Some("of ") {
        start -= 3;
    }
    push_bbcode_segment(out, &text[..start]);
    let _ = write!(out, "[color={color}]");
    push_bbcode_segment(out, &text[start..end]);
    out.push_str("[/color]");
    push_bbcode_segment(out, &text[end..]);
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

//! 论坛目录分页与 BBCode 渲染；价格及资格仍由权威拍卖快照决定。

mod colors;
mod countdown;
mod log;
mod settlement;
#[cfg(test)]
mod tests;
mod text;
mod validation;

use auction_application::forum::{
    EquipmentColorRule, FORUM_CATALOG_PAGE_SIZE, ForumAuctionSnapshot, ForumCurrentBidSource,
    ForumRenderError, ForumRenderedPost, ForumRenderedThread, ForumRenderer, ForumSystemLogEntry,
    MAX_FORUM_CATALOG_LOTS, MAX_FORUM_RECONCILIATION_TITLES, forum_catalog_page_marker,
};
use auction_application::public::PublicEquipmentGameLink;
use auction_domain::{AuctionId, AuctionStatus, SettlementWorld, valid_text};
use sha2::{Digest, Sha256};

use crate::clients::ehentai::{
    MAXIMUM_FORUM_BODY_BYTES, MAXIMUM_FORUM_TITLE_RUNES, PRODUCTION_ENDPOINT,
};
use colors::colored_equipment_name;
use text::{
    bbcode_text, category_label, category_order, format_credits, format_forum_time, push_fmt,
};
use validation::{valid_forum_post_id, valid_lot, valid_public_url};

const MAX_TITLE_BYTES: usize = 512;
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
    push_fmt(
        &mut body,
        format_args!(
            "[size=5][b][HV Auction System] {world_title} World Equipment Auction - Session {:03}[/b][/size]\n",
            snapshot.sequence
        ),
    );
    // 论坛回读会删除开列表前、外层闭列表后的换行；直接生成其稳定形状，避免放宽正文比较。
    body.push_str(FORUM_SEPARATOR);
    body.push_str("[list]\n[*][b]World:[/b] ");
    body.push_str(snapshot.world.as_str());
    body.push('\n');
    push_fmt(
        &mut body,
        format_args!(
            "[*][b]Bidding opens:[/b] {}\n",
            format_forum_time(snapshot.scheduled_open_at)
        ),
    );
    if let Some(opened_at) = snapshot.opened_at {
        push_fmt(&mut body, format_args!("[*][b]Opened:[/b] {}\n", format_forum_time(opened_at)));
    }
    if let Some(scheduled_end_at) = snapshot.scheduled_end_at {
        push_fmt(
            &mut body,
            format_args!("[*][b]Bidding deadline:[/b] {}\n", format_forum_time(scheduled_end_at)),
        );
    }
    if let Some(public_url) = snapshot.public_url.as_deref() {
        // 论坛会截短纯 URL 链接文字并把省略号写回 BBCode；内层粗体保留完整地址及回读证明。
        push_fmt(
            &mut body,
            format_args!(
                "[*][b]Auction address:[/b] [url={public_url}][b]{}[/b][/url]\n",
                bbcode_text(public_url)
            ),
        );
    }
    let system_url = format!("{PRODUCTION_ENDPOINT}/index.php?showtopic=301946");
    push_fmt(
        &mut body,
        format_args!("[*][b]About system:[/b] [url={system_url}]{system_url}[/url]\n"),
    );
    // 规则区是玩家可见的现行行为摘要，语义以 docs/guide/forum.md 为准：notify 只标记 lot，
    // 提醒渠道由账号偏好决定，mm/pm 参数不改变渠道。只沿用已验证可原样回读的列表与
    // [code] 形状，避免固定楼层正文比较因论坛规范化而失配。
    body.push_str(
        "[/list]\
         [size=3][b]Auction Rules and Bidding[/b][/size]\
         [list]\n\
         [*]The public auction page is authoritative for prices, leaders and deadlines.\n\
         [*][b]Bidding commands:[/b]\
         [list]\n\
         [*]Post one command per line.\n\
         [*]Example: [code]One01 200k[/code]\n\
         [*]Proxy bidding is available only on the public auction page; forum proxy lines are \
         ignored.\n\
         [*]Outbid notices cover marked lots. Accepted bids and new proxy authorizations mark \
         the lot; you can also mark lots on the public auction page or post lot codes followed \
         by notify:\n\
         [code]One01 notify[/code]\n\
         [*]Notices use the channels chosen in My information / Outbid notifications on the \
         public auction page: MoogleMail by default; forum PM needs a linked forum account and \
         is only for outbid notices, not settlement or COD. notify mm and notify pm work the \
         same as notify and do not change the channels.\n\
         [*]Amounts are Credits and must use the 1k grid. Extra words make a command invalid.\n\
         [/list]\n\
         [/list]",
    );
    push_fmt(&mut body, format_args!("{FORUM_SEPARATOR}\n"));

    let page_size = snapshot.catalog_layout.page_size();
    let page_count = lots.len().div_ceil(page_size);
    if page_count > 1 {
        write_catalog_navigation(&mut body, page_count);
    }
    let mut pages = Vec::with_capacity(page_count);
    let nominal_end = snapshot.scheduled_end_at;
    let mut last_category = None::<String>;
    for (index, (lot, equipment_link)) in lots.into_iter().enumerate() {
        if index > 0 && index % page_size == 0 {
            finish_catalog_page(&mut body, snapshot.auction_id, pages.len() + 1)?;
            pages.push(ForumRenderedPost { body: std::mem::take(&mut body) });
            push_fmt(
                &mut body,
                format_args!(
                    "[size=3][b]{world_title} Equipment Auction - Session {:03} - Catalog {}/{}[/b][/size]\n",
                    snapshot.sequence,
                    pages.len() + 1,
                    page_count
                ),
            );
            write_catalog_navigation(&mut body, page_count);
            // 跨帖重复分类标题；不改变分类编号，也不把续页当成上一页的裸文本尾部。
            last_category = None;
        }
        if last_category.as_deref() != Some(lot.category.as_str()) {
            if last_category.is_some() {
                // 分类之间保留两行空白，避免长目录中标题和上一类最后一件装备粘连。
                body.push_str("\n\n");
            }
            // 分类标题与下一行的 `[code]` 行首是论坛回读解析契约，样式调整不能改变其纯文本形状。
            push_fmt(
                &mut body,
                format_args!("[size=3][b]{}[/b][/size]\n", category_label(&lot.category)),
            );
            last_category = Some(lot.category.clone());
        }
        let equipment_url = equipment_link.into_url();
        push_fmt(
            &mut body,
            format_args!(
                "[b][{}][/b] [url={}][b]{}[/b][/url]",
                lot.code,
                equipment_url,
                colored_equipment_name(&lot, colors)
            ),
        );
        // 装备事实直接追加到最终帖子，避免每件装备分配字段 Vec、临时字符串和 join 副本。
        if lot.equipment_level.is_some()
            || lot.primary_attribute_initials.is_some()
            || lot.avg_roll_tenths.is_some()
        {
            body.push_str(" (");
            let mut separator = "";
            if let Some(level) = lot.equipment_level {
                push_fmt(&mut body, format_args!("Lv.{level}"));
                separator = ", ";
            }
            if let Some(initials) = lot.primary_attribute_initials.as_deref() {
                body.push_str(separator);
                body.push_str(initials);
                separator = ", ";
            }
            if let Some(tenths) = lot.avg_roll_tenths {
                push_fmt(
                    &mut body,
                    format_args!("{separator}Ave: {}.{:01}%", tenths / 10, tenths % 10),
                );
            }
            body.push(')');
        }
        // seller/start/current 的纯文本标签由论坛目录回读解析器消费，必须保持稳定。
        push_fmt(
            &mut body,
            format_args!(
                " (seller: {}) (start: {})",
                bbcode_text(&lot.seller),
                format_credits(lot.start_price)
            ),
        );
        // 首笔合法竞价可以恰好等于起拍价；只要已有领先者，也必须展示当前事实与来源。
        if lot.current_price > lot.start_price || lot.current_bidder.is_some() {
            push_fmt(
                &mut body,
                format_args!(" | [b]Current: {}[/b]", format_credits(lot.current_price)),
            );
            if let Some(bidder) = lot.current_bidder.as_deref() {
                push_fmt(&mut body, format_args!(" by {}", bbcode_text(bidder)));
                match lot.current_bid_source.as_ref() {
                    Some(ForumCurrentBidSource::Forum { post_number, thread_id, post_id }) => {
                        push_fmt(
                            &mut body,
                            format_args!(
                                " ([url={PRODUCTION_ENDPOINT}/index.php?showtopic={thread_id}&p={post_id}#entry{post_id}]#{post_number}[/url])"
                            ),
                        );
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
            push_fmt(&mut body, format_args!(" | Ends: {}", format_forum_time(effective_end_at)));
        }
        if lot.status != "live" {
            push_fmt(&mut body, format_args!(" | [b]Status:[/b] {}", bbcode_text(&lot.status)));
        }
        body.push('\n');
        if body.len() > MAXIMUM_FORUM_BODY_BYTES {
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

/// 首帖与续页使用同一页尾和独立字节上限，任何一页超限都在远端写入前失败。
fn finish_catalog_page(
    body: &mut String,
    auction_id: AuctionId,
    post_number: usize,
) -> Result<(), ForumRenderError> {
    push_fmt(body, format_args!("{FORUM_SEPARATOR}\n"));
    body.push_str(
        "[size=1][color=#666666][i]The public auction page remains authoritative for live auction state.[/i][/color][/size]\n",
    );
    push_fmt(
        body,
        format_args!(
            "[size=1][color=transparent]{}[/color][/size]\n",
            forum_catalog_page_marker(auction_id, post_number)
        ),
    );
    if body.len() > MAXIMUM_FORUM_BODY_BYTES {
        return Err(ForumRenderError::BodyTooLarge);
    }
    Ok(())
}

fn write_catalog_navigation(body: &mut String, page_count: usize) {
    push_fmt(
        body,
        format_args!(
            "[size=1]Equipment: posts #1-#{page_count} (at most {FORUM_CATALOG_PAGE_SIZE} per post). Updates: #{}. Bids: #{} onward.[/size]\n\n",
            page_count + 1,
            page_count + 2
        ),
    );
}

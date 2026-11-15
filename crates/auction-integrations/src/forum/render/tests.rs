//! 论坛目录与系统更新楼渲染的形状回归：行尾换行、分页导航与页尾是回读解析契约，
//! 正文上限与发帖校验共用同一常量。

use auction_application::forum::{
    EquipmentColorRule, FORUM_CATALOG_PAGE_SIZE, ForumAuctionSnapshot, ForumCatalogLayout,
    ForumCurrentBidSource, ForumLotSettlement, ForumLotSnapshot, ForumRenderError, ForumRenderer,
    ForumSystemLogEntry, forum_catalog_page_marker, forum_system_log_marker,
};
use auction_application::public::{PublicEquipmentGameLink, PublicEquipmentLinkTarget};
use auction_domain::equipment::taxonomy::EquipmentCategory;
use auction_domain::{AuctionId, AuctionStatus, EquipmentId, SettlementWorld};
use chrono::{DateTime, TimeZone, Utc};
use uuid::Uuid;

use super::text::{category_label, category_order};
use super::{FORUM_SEPARATOR, ForumThreadRenderer};
use crate::clients::ehentai::MAXIMUM_FORUM_BODY_BYTES;

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 7, hour, 0, 0).single().expect("fixture time")
}

fn auction_id() -> AuctionId {
    AuctionId::from_uuid(Uuid::from_u128(0x0f3c_8f60_1111_4a2b_8a1d_6f2f_5f4b_9c11))
}

fn lot(index: usize) -> (ForumLotSnapshot, PublicEquipmentGameLink) {
    let external_id = 100_000 + index;
    let snapshot = ForumLotSnapshot {
        code: format!("One{:02}", index + 1),
        category: "One-Handed".to_owned(),
        name: "Legendary Savage Rapier of Slaughter".to_owned(),
        equipment_quality: "Legendary".to_owned(),
        equipment_type: "Rapier".to_owned(),
        equipment_prefix: "Savage".to_owned(),
        equipment_suffix: "Slaughter".to_owned(),
        equipment_level: Some(500),
        primary_attribute_initials: Some("DEX".to_owned()),
        avg_roll_tenths: Some(853),
        seller: "Seller [A]".to_owned(),
        start_price: 100_000,
        current_price: 100_000,
        status: "live".to_owned(),
        current_bidder: None,
        current_bid_source: None,
        effective_end_at: None,
        settlement: None,
        equipment_link_target: PublicEquipmentLinkTarget {
            lot_id: Uuid::from_u128(index as u128 + 1),
            equipment_id: EquipmentId::from_uuid(Uuid::from_u128(10_000 + index as u128)),
            world_id: "main".to_owned(),
            projected_game_equipment_url: format!("https://hentaiverse.org/equip/{external_id}"),
        },
    };
    let link = PublicEquipmentGameLink::new(format!(
        "https://hentaiverse.org/equip/{external_id}/0123456789"
    ))
    .expect("fixture equipment link");
    (snapshot, link)
}

fn snapshot(
    layout: ForumCatalogLayout,
    state: AuctionStatus,
    lots: Vec<ForumLotSnapshot>,
) -> ForumAuctionSnapshot {
    ForumAuctionSnapshot {
        auction_id: auction_id(),
        catalog_layout: layout,
        world: SettlementWorld::Main,
        sequence: 7,
        state_version: 3,
        state,
        scheduled_open_at: at(8),
        opened_at: Some(at(9)),
        scheduled_end_at: Some(at(12)),
        public_url: Some("https://auction.example.com/".to_owned()),
        lots,
    }
}

/// 151 件拍品分两帖；首件带论坛出价来源与延长截止，第二件带代理出价与非 live 状态。
fn paged_fixture() -> (ForumAuctionSnapshot, Vec<PublicEquipmentGameLink>) {
    let (mut lots, links): (Vec<_>, Vec<_>) = (0..=FORUM_CATALOG_PAGE_SIZE).map(lot).unzip();
    lots[0].current_price = 150_000;
    lots[0].current_bidder = Some("Bidder".to_owned());
    lots[0].current_bid_source = Some(ForumCurrentBidSource::Forum {
        post_number: 5,
        thread_id: "302133".to_owned(),
        post_id: "6987999".to_owned(),
    });
    lots[0].effective_end_at = Some(at(13));
    lots[1].current_price = 1_250_000;
    lots[1].current_bidder = Some("Proxy Bidder".to_owned());
    lots[1].current_bid_source = Some(ForumCurrentBidSource::Proxy);
    lots[1].status = "withdrawn".to_owned();
    (snapshot(ForumCatalogLayout::PagedV1, AuctionStatus::Live, lots), links)
}

/// 已结算场次：成交件带结算尾注与 Peerless 彩虹名，流拍件按标色规则只给后缀上色。
fn settled_fixture() -> (ForumAuctionSnapshot, Vec<PublicEquipmentGameLink>, Vec<EquipmentColorRule>)
{
    let (mut sold, sold_link) = lot(0);
    sold.name = "Peerless Savage Rapier of Slaughter".to_owned();
    sold.equipment_quality = "Peerless".to_owned();
    sold.current_price = 300_000;
    sold.current_bidder = Some("Winner".to_owned());
    sold.status = "sold".to_owned();
    sold.settlement = Some(ForumLotSettlement {
        leader_held_nanos: 3 * 3_600_000_000_000,
        adjustment_ppm: 15_000,
    });
    let (mut unsold, unsold_link) = lot(1);
    unsold.status = "unsold".to_owned();
    let rules = vec![EquipmentColorRule {
        category: String::new(),
        equipment_type: String::new(),
        quality: String::new(),
        categories: Vec::new(),
        equipment_types: Vec::new(),
        qualities: Vec::new(),
        level_min: 0,
        level_max: 0,
        prefixes: Vec::new(),
        suffixes: vec!["Slaughter".to_owned()],
        prefix_color: "#00aa00".to_owned(),
        suffix_color: "#aa0000".to_owned(),
        combined_color: "#0000aa".to_owned(),
    }];
    (
        snapshot(ForumCatalogLayout::LegacySingle, AuctionStatus::Settled, vec![sold, unsold]),
        vec![sold_link, unsold_link],
        rules,
    )
}

fn log_entry(post_number: usize, code: &str, ignored: bool) -> ForumSystemLogEntry {
    ForumSystemLogEntry {
        post_number,
        post_id: format!("{}", 7_000_000 + post_number),
        line_number: post_number.is_multiple_of(2).then_some(2),
        code: code.to_owned(),
        message: format!("fixture [message] {post_number}"),
        ignored,
    }
}

fn log_entries() -> Vec<ForumSystemLogEntry> {
    let mut entries =
        vec![log_entry(5, "command_pending", false), log_entry(6, "ignored_post", true)];
    entries.extend((10..115).map(|post_number| log_entry(post_number, "invalid_amount", false)));
    entries
}

#[test]
fn paged_catalog_keeps_line_shapes_navigation_and_page_footer() {
    let (snapshot, links) = paged_fixture();
    let thread = ForumThreadRenderer::default().render(snapshot, links).expect("render catalog");
    assert_eq!(thread.catalog_continuation.len(), 1);
    let navigation = "[size=1]Equipment: posts #1-#2 (at most 150 per post). Updates: #3. \
                      Bids: #4 onward.[/size]\n\n";
    let first = &thread.body;
    assert!(first.starts_with(
        "[img]https://auction.example.com/api/v1/auctions/0f3c8f60-1111-4a2b-8a1d-6f2f5f4b9c11\
         /countdown/list.svg#.png[/img] [img]https://auction.example.com/api/v1/auctions/\
         0f3c8f60-1111-4a2b-8a1d-6f2f5f4b9c11/countdown/auction.svg#.png[/img]\n\
         [size=5][b][HV Auction System] Main World Equipment Auction - Session 007[/b][/size]\n"
    ));
    assert!(first.contains(
        "[*][b]Bidding opens:[/b] 2026-11-07 08:00:00 UTC\n\
         [*][b]Opened:[/b] 2026-11-07 09:00:00 UTC\n\
         [*][b]Bidding deadline:[/b] 2026-11-07 12:00:00 UTC\n\
         [*][b]Auction address:[/b] [url=https://auction.example.com/][b]https://auction.example.com/[/b][/url]\n\
         [*][b]About system:[/b] [url=https://forums.e-hentai.org/index.php?showtopic=301946]\
         https://forums.e-hentai.org/index.php?showtopic=301946[/url]\n"
    ));
    assert!(first.contains(&format!(
        "[/list]{FORUM_SEPARATOR}\n{navigation}[size=3][b]One-Handed[/b][/size]\n"
    )));
    assert!(first.contains(
        "[b][One01][/b] [url=https://hentaiverse.org/equip/100000/0123456789][b]Legendary Savage \
         Rapier of Slaughter[/b][/url] (Lv.500, DEX, Ave: 85.3%) (seller: Seller ［A］) (start: 100k) \
         | [b]Current: 150k[/b] by Bidder ([url=https://forums.e-hentai.org/index.php?showtopic=302133\
         &p=6987999#entry6987999]#5[/url]) | Ends: 2026-11-07 13:00:00 UTC\n"
    ));
    assert!(first.contains(
        "(start: 100k) | [b]Current: 1.25m[/b] by Proxy Bidder (proxy) | [b]Status:[/b] withdrawn\n"
    ));
    let footer = |post_number: usize| {
        format!(
            "{FORUM_SEPARATOR}\n[size=1][color=#666666][i]The public auction page remains \
             authoritative for live auction state.[/i][/color][/size]\n\
             [size=1][color=transparent]{}[/color][/size]\n",
            forum_catalog_page_marker(auction_id(), post_number)
        )
    };
    assert!(first.ends_with(&footer(1)));
    let second = &thread.catalog_continuation[0].body;
    assert!(second.starts_with(&format!(
        "[size=3][b]Main Equipment Auction - Session 007 - Catalog 2/2[/b][/size]\n{navigation}\
         [size=3][b]One-Handed[/b][/size]\n[b][One151][/b] "
    )));
    assert!(second.ends_with(&footer(2)));
    for body in [first, second] {
        assert!(body.len() <= MAXIMUM_FORUM_BODY_BYTES, "渲染上限即发帖校验上限");
    }
}

/// 首帖规则区是玩家可见文本，逐字固定：start 只开未被出价的 lot；notify 只标记 lot、渠道由账号
/// 偏好决定且 mm/pm 不改变渠道；规则区只出现在首帖，续页不重复。
#[test]
fn catalog_rules_describe_current_notify_and_channel_semantics() {
    let (snapshot, links) = paged_fixture();
    let thread = ForumThreadRenderer::default().render(snapshot, links).expect("render catalog");
    let rules = "[/list][size=3][b]Auction Rules and Bidding[/b][/size][list]\n\
                 [*]The public auction page is authoritative for prices, leaders and deadlines.\n\
                 [*][b]Bidding commands:[/b][list]\n\
                 [*]Post one command per line.\n\
                 [*]Example: [code]One01 200k[/code]\n\
                 [*]A lot with no bids yet can be opened at its starting price: \
                 [code]One01 start[/code]\n\
                 [*]Proxy bidding is available only on the public auction page; forum proxy \
                 lines are ignored.\n\
                 [*]Outbid notices cover marked lots. Accepted bids and new proxy authorizations \
                 mark the lot; you can also mark lots on the public auction page or post lot codes \
                 followed by notify:\n\
                 [code]One01 notify[/code]\n\
                 [*]Notices use the channels chosen in My information / Outbid notifications on \
                 the public auction page: MoogleMail by default; forum PM needs a linked forum \
                 account and is only for outbid notices, not settlement or COD. notify mm and \
                 notify pm work the same as notify and do not change the channels.\n\
                 [*]Amounts are Credits and must use the 1k grid. Extra words make a command \
                 invalid.\n\
                 [/list]\n\
                 [/list]";
    assert!(thread.body.contains(&format!("{rules}{FORUM_SEPARATOR}\n")));
    assert_eq!(thread.body.matches("Auction Rules and Bidding").count(), 1);
    assert!(!thread.catalog_continuation[0].body.contains("Auction Rules and Bidding"));
}

#[test]
fn settled_catalog_renders_annotation_and_colored_names() {
    let (snapshot, links, rules) = settled_fixture();
    let thread =
        ForumThreadRenderer::with_colors(rules).render(snapshot, links).expect("render catalog");
    let rainbow = "[color=#ff0000]P[/color][color=#ff7f00]e[/color][color=#c3c300]e[/color]\
                   [color=#00ff00]r[/color][color=#00ffff]l[/color][color=#7171ff]e[/color]\
                   [color=#8b00ff]s[/color][color=#ff0000]s[/color] ";
    assert!(thread.body.contains(&format!(
        "[b][One01][/b] [url=https://hentaiverse.org/equip/100000/0123456789][b]{rainbow}Savage \
         Rapier [color=#aa0000]of Slaughter[/color][/b][/url]"
    )));
    assert!(thread.body.contains(
        "| [b]Current: 300k[/b] by Winner; winning for 3.0h / 4.0h; +1.5% | [b]Status:[/b] sold\n"
    ));
    assert!(thread.body.contains(
        "[b][One02][/b] [url=https://hentaiverse.org/equip/100001/0123456789][b]Legendary Savage \
         Rapier [color=#aa0000]of Slaughter[/color][/b][/url]"
    ));
}

#[test]
fn oversized_single_post_catalog_fails_before_any_remote_write() {
    let (mut lots, links): (Vec<_>, Vec<_>) = (0..200).map(lot).unzip();
    for lot in &mut lots {
        lot.seller = "S".repeat(256);
    }
    let snapshot = snapshot(ForumCatalogLayout::LegacySingle, AuctionStatus::Live, lots);
    assert_eq!(
        ForumThreadRenderer::default().render(snapshot, links).err(),
        Some(ForumRenderError::BodyTooLarge)
    );
}

#[test]
fn system_log_keeps_header_overflow_note_and_marker() {
    let post = ForumThreadRenderer::default()
        .render_system_log(auction_id(), 2, 120, log_entries())
        .expect("render system log");
    let body = post.body;
    assert!(body.starts_with(
        "Updated through [b][size=5]#120[/size][/b] — [b]1[/b] pending, [b]105[/b] warnings, \
         [b]1[/b] ignored.\n[size=1][color=#666666]Accepted bids and current prices appear in \
         posts #1-#2. Bids begin at post #4."
    ));
    assert!(body.contains(
        "\n[center][b][size=3]Pending — result not confirmed[/size][/b][/center]\n\n\
         [url=https://forums.e-hentai.org/index.php?act=findpost&pid=7000005][b]#5[/b][/url] \
         [color=#777777]command_pending[/color] — fixture ［message］ 5\n"
    ));
    assert!(body.contains(
        "[url=https://forums.e-hentai.org/index.php?act=findpost&pid=7000114][b]#114[/b][/url] \
         [color=#777777]line 2 · invalid_amount[/color] — fixture ［message］ 114\n"
    ));
    assert!(body.contains("\n[size=1]Showing 100 of 107 diagnostics to fit the forum post limit."));
    assert!(body.ends_with(&format!(
        "[size=1][color=transparent]{}[/color][/size]\n",
        forum_system_log_marker(auction_id())
    )));
    assert!(body.len() <= MAXIMUM_FORUM_BODY_BYTES);
}

/// 目录分组顺序与标题以领域七大类为准：规范名称与 lot 代码前缀都识别（不区分大小写），轻甲与重甲
/// 保留论坛已发布的 “Armor” 标题，未知分类排在最后并原样转义。
#[test]
fn catalog_categories_follow_the_domain_order_and_forum_labels() {
    let labels = EquipmentCategory::ALL.map(|category| category_label(category.as_str()));
    assert_eq!(
        labels,
        ["One-Handed", "Two-Handed", "Staff", "Shield", "Cloth", "Light Armor", "Heavy Armor"]
    );
    for (index, category) in EquipmentCategory::ALL.into_iter().enumerate() {
        assert_eq!(category_order(category.as_str()), index + 1);
        assert_eq!(category_order(&category.as_str().to_ascii_lowercase()), index + 1);
        assert_eq!(category_order(&category.lot_code_prefix().to_ascii_uppercase()), index + 1);
    }
    assert_eq!(category_order("Ring"), usize::MAX);
    assert_eq!(category_label("Ring"), "Ring");
}

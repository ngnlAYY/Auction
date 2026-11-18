//! Public URL 查询状态、筛选规范化和浏览器历史辅助。

mod auction;
mod commit;
mod filter;
#[cfg(any(target_arch = "wasm32", test))]
mod parse;

use auction_web_shared::QueryWriteMode;
use auction_web_shared::query::{location_parts, query_location};
use dioxus::prelude::*;
use url::form_urlencoded;

use super::{public_page_within_offset, stored_public_page_size};
use crate::api::{CurrentAuction, HistoricalAuction};
#[cfg(any(target_arch = "wasm32", test))]
pub(super) use parse::parse_public_query;

pub(crate) use auction::{AuctionParameter, AuctionSelection};
pub(crate) use commit::{QueryCommit, commit_auction_selection, update_public_query_state};
pub(crate) use filter::FilterParameter;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveConnectionState {
    Connecting,
    Degraded,
    Live,
    Manual,
}

/// 公开拍卖看板的 URL 状态；筛选和分页必须能被刷新、分享和返回操作恢复。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicBoardQuery {
    pub(crate) world: FilterParameter,
    #[doc(hidden)]
    pub(crate) auction: auction::StoredAuction,
    pub(crate) category: FilterParameter,
    pub(crate) equipment_group: Option<String>,
    /// 非法等级参数保留原文供筛选栏修正，不能先丢条件再发出宽查询。
    pub(crate) max_level: Option<String>,
    pub(crate) include_unleveled: String,
    pub(crate) status: Option<String>,
    pub(crate) history_name: Option<String>,
    pub(crate) history_from: Option<String>,
    pub(crate) history_to: Option<String>,
    pub(crate) history_min_price: Option<String>,
    pub(crate) history_max_price: Option<String>,
    pub(crate) history_buyer: Option<String>,
    pub(crate) history_seller: Option<String>,
    /// 成交历史独立的全局排序与本页展示方式，不复用目录的 type/status。
    pub(crate) history_sort: String,
    pub(crate) history_group: String,
    pub(crate) bid_log_lot: Option<String>,
    pub(crate) page: i64,
    pub(crate) page_size: i64,
    /// 补偿池展开流水的独立分页；批次分页与流水分页不能互相覆盖。
    pub(crate) movement_page: i64,
    pub(crate) movement_page_size: i64,
    /// 是否从迁移期间写出的 `bidlog_page` 别名恢复了竞价日志页码。
    pub(crate) bid_log_page_alias: bool,
    pub(crate) bid_page: i64,
    pub(crate) proxy_page: i64,
    /// 当前页面不拥有的 query 对必须原样保留，避免筛选或翻页时删除外部入口参数。
    pub(crate) passthrough: Vec<(String, String)>,
}

pub(crate) type PreviewHistoryLoad =
    Result<Option<(CurrentAuction, Vec<HistoricalAuction>)>, String>;

/// 浏览器 pathname/query 变化版本；查询驱动页面通过独立类型订阅，避免与数据刷新信号混淆。
#[derive(Clone, Copy)]
pub(crate) struct PublicLocationRevision(pub(crate) Signal<u64>);

impl Default for PublicBoardQuery {
    fn default() -> Self {
        Self {
            world: FilterParameter::Unspecified,
            auction: auction::StoredAuction::default(),
            category: FilterParameter::Unspecified,
            equipment_group: None,
            max_level: None,
            include_unleveled: "true".to_owned(),
            status: None,
            history_name: None,
            history_from: None,
            history_to: None,
            history_min_price: None,
            history_max_price: None,
            history_buyer: None,
            history_seller: None,
            history_sort: "closed_desc".to_owned(),
            history_group: "equipment".to_owned(),
            bid_log_lot: None,
            page: 1,
            page_size: stored_public_page_size(false),
            movement_page: 1,
            movement_page_size: stored_public_page_size(true),
            bid_log_page_alias: false,
            bid_page: 1,
            proxy_page: 1,
            passthrough: Vec::new(),
        }
    }
}

impl PublicBoardQuery {
    /// 启动预取与正文使用同一门禁，待修正的地址不能发目录查询。
    pub(crate) fn catalog_request_category(&self) -> Option<Option<&str>> {
        if matches!(self.auction_parameter(), AuctionParameter::Invalid(_))
            || !public_page_within_offset(self.page, self.page_size)
        {
            return None;
        }
        self.category.catalog_category()
    }
}

pub(crate) fn read_public_query() -> PublicBoardQuery {
    #[cfg(any(target_arch = "wasm32", test))]
    {
        let location = current_public_location();
        let (path, search, _) = location_parts(&location);
        parse_public_query(path, search)
    }

    #[cfg(not(any(target_arch = "wasm32", test)))]
    PublicBoardQuery::default()
}

fn current_public_location() -> String {
    if let Some(location) = query_location() {
        return location.current();
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let location = window.location();
        return format!(
            "{}{}{}",
            location.pathname().unwrap_or_else(|_| "/".to_owned()),
            location.search().unwrap_or_default(),
            location.hash().unwrap_or_default(),
        );
    }
    "/".to_owned()
}

pub(crate) fn normalize_world_query(value: &str) -> Option<String> {
    match value.trim() {
        "main" => Some("main".to_owned()),
        "isekai" => Some("isekai".to_owned()),
        _ => None,
    }
}

pub(crate) fn public_query_string(query: &PublicBoardQuery) -> String {
    let pairs = public_query_pairs(query);
    if pairs.is_empty() {
        return String::new();
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in &pairs {
        serializer.append_pair(key, value);
    }
    format!("?{}", serializer.finish())
}

/// 序列化公开端查询状态；数量即使为 10 也显式保留，防止本地偏好改变分享及历史语义。
pub(crate) fn public_query_pairs(query: &PublicBoardQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    pairs
        .extend(query.world.query_values().iter().map(|value| ("world".to_owned(), value.clone())));
    pairs.extend(
        query
            .auction_parameter()
            .query_values()
            .iter()
            .map(|value| ("auction".to_owned(), value.clone())),
    );
    if let Some(equipment_group) = query.equipment_group.as_deref() {
        pairs.push(("equipment_group".to_owned(), equipment_group.to_owned()));
    }
    if let Some(max_level) = query.max_level.as_deref() {
        pairs.push(("max_level".to_owned(), max_level.to_owned()));
    }
    if query.include_unleveled != "true" {
        pairs.push(("include_unleveled".to_owned(), query.include_unleveled.clone()));
    }
    pairs.extend(
        query.category.query_values().iter().map(|value| ("type".to_owned(), value.clone())),
    );
    if let Some(status) = query.status.as_deref() {
        pairs.push(("status".to_owned(), status.to_owned()));
    }
    for (key, value) in [
        ("name", query.history_name.as_deref()),
        ("from", query.history_from.as_deref()),
        ("to", query.history_to.as_deref()),
        ("min_price", query.history_min_price.as_deref()),
        ("max_price", query.history_max_price.as_deref()),
        ("buyer", query.history_buyer.as_deref()),
        ("seller", query.history_seller.as_deref()),
        ("lot", query.bid_log_lot.as_deref()),
    ] {
        if let Some(value) = value {
            pairs.push((key.to_owned(), value.to_owned()));
        }
    }
    if query.history_sort != "closed_desc" {
        pairs.push(("history_sort".to_owned(), query.history_sort.clone()));
    }
    if query.history_group != "equipment" {
        pairs.push(("history_group".to_owned(), query.history_group.clone()));
    }
    if query.page > 1 {
        pairs.push(("page".to_owned(), query.page.to_string()));
    }
    pairs.push(("page_size".to_owned(), query.page_size.to_string()));
    if query.movement_page > 1 {
        pairs.push(("movement_page".to_owned(), query.movement_page.to_string()));
    }
    pairs.push(("movement_page_size".to_owned(), query.movement_page_size.to_string()));
    if query.bid_page > 1 {
        pairs.push(("bid_page".to_owned(), query.bid_page.to_string()));
    }
    if query.proxy_page > 1 {
        pairs.push(("proxy_page".to_owned(), query.proxy_page.to_string()));
    }
    // 整段重置与普通链接的场次都只来自三态，透传值不能另行提供场次。
    pairs.extend(
        query
            .passthrough
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "auction" | "world" | "type"))
            .cloned(),
    );
    pairs
}

/// 已应用查询由地址驱动，表单草稿由各页面持有。
pub(crate) struct PublicQueryState {
    pub(crate) state: Signal<PublicBoardQuery>,
}

/// 挂载与地址栏双向同步的公开端查询状态。
pub(crate) fn use_public_query_state() -> PublicQueryState {
    let mut state = use_signal(read_public_query);
    let location_revision = try_consume_context::<PublicLocationRevision>();
    let router = dioxus::prelude::router();
    use_effect(move || {
        // 从同一个地址恢复 Link、查询直接写回和浏览器前进/后退。
        let _ = router.full_route_string();
        if let Some(PublicLocationRevision(revision)) = location_revision {
            let _ = revision();
        }
        let current = read_public_query();
        if *state.peek() != current {
            state.set(current);
        }
    });
    PublicQueryState { state }
}

/// 当前地址栏 pathname；非 WASM 构建缺省为 `/`。
pub(crate) fn current_public_pathname() -> String {
    location_parts(&current_public_location()).0.to_owned()
}

/// 首页重置等显式导航替换完整查询。
pub(crate) fn set_public_query(query: &PublicBoardQuery, replace: bool) {
    let Some(location) = query_location() else { return };
    let mode = if replace { QueryWriteMode::Replace } else { QueryWriteMode::Push };
    location.write(&public_href(&current_public_pathname(), query), mode);
}

pub(crate) fn public_href(path: &str, query: &PublicBoardQuery) -> String {
    format!("{path}{}", public_query_string(query))
}

/// 构建明确选择或清除场次的原生导航链接，保留来源筛选。
pub(crate) fn public_href_for_auction(
    path: &str,
    query: PublicBoardQuery,
    auction_id: Option<String>,
) -> String {
    public_href(path, &query.with_auction_values(auction_id))
}

/// 首页入口不冻结数量：导航时读取最新本地偏好，避免外壳旧链接覆盖刚保存的选择。
pub(crate) fn public_auction_home_href(world: &str) -> String {
    if world == "isekai" { "/?world=isekai" } else { "/" }.to_owned()
}

/// 生成当前世界的拍卖首页查询；同路由导航可直接用它替换历史视图状态。
pub(crate) fn public_auction_home_query(world: &str) -> PublicBoardQuery {
    // 只保留目标世界与本地数量偏好，避免从其他页面带回不属于首页的筛选和页码。
    PublicBoardQuery {
        world: (world == "isekai").then(|| "isekai".to_owned()).into(),
        ..PublicBoardQuery::default()
    }
}

/// 收敛公开端别名时保留原地址的查询与片段，替换当前条目使后退不再进入别名。
pub(crate) fn redirect_public_alias(path: &'static str) -> Element {
    redirect_public_alias_if(path, || true)
}

/// 写入前复核跳转条件，避免过期别名覆盖更新后的地址。
pub(crate) fn redirect_public_alias_if(path: &'static str, may_redirect: fn() -> bool) -> Element {
    let source = current_public_pathname();
    use_effect(move || {
        let location = current_public_location();
        let (pathname, search, hash) = location_parts(&location);
        if pathname == source && may_redirect() {
            let _ = navigator().replace(format!("{path}{search}{hash}"));
        }
    });
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect()
    }

    #[test]
    fn default_query_state_serializes_only_stable_page_sizes() {
        let query = PublicBoardQuery::default();
        assert_eq!(query.include_unleveled, "true");
        assert_eq!(query.history_sort, "closed_desc");
        assert_eq!(query.history_group, "equipment");
        assert_eq!(
            public_query_pairs(&query),
            pairs(&[("page_size", "20"), ("movement_page_size", "20")])
        );
    }

    #[test]
    fn canonical_pairs_follow_the_documented_order_and_keep_passthrough() {
        let mut query = PublicBoardQuery {
            world: Some("main".to_owned()).into(),
            category: Some("one".to_owned()).into(),
            status: Some("live".to_owned()),
            history_sort: "price_desc".to_owned(),
            history_group: "buyer".to_owned(),
            page: 2,
            ..PublicBoardQuery::default()
        };
        query.passthrough = pairs(&[("utm", "a b"), ("auction", "unexpected")]);
        assert_eq!(
            public_query_pairs(&query),
            pairs(&[
                ("world", "main"),
                ("type", "one"),
                ("status", "live"),
                ("history_sort", "price_desc"),
                ("history_group", "buyer"),
                ("page", "2"),
                ("page_size", "20"),
                ("movement_page_size", "20"),
                ("utm", "a b"),
            ])
        );

        let encoded = public_query_string(&query);
        assert!(encoded.starts_with('?'));
        assert!(encoded.contains("utm=a+b"));
        assert!(!encoded.contains("history_sort=closed_desc"));
    }

    #[test]
    fn omitted_defaults_are_not_written_back_and_invalid_world_values_are_dropped() {
        let query = PublicBoardQuery {
            page: 1,
            movement_page: 1,
            bid_page: 1,
            proxy_page: 1,
            ..PublicBoardQuery::default()
        };
        assert!(!public_query_pairs(&query).iter().any(|(key, _)| key == "page"));

        let query = PublicBoardQuery {
            include_unleveled: "false".to_owned(),
            ..PublicBoardQuery::default()
        };
        assert!(
            public_query_pairs(&query)
                .contains(&("include_unleveled".to_owned(), "false".to_owned()))
        );

        assert_eq!(normalize_world_query(" main "), Some("main".to_owned()));
        assert_eq!(normalize_world_query("isekai"), Some("isekai".to_owned()));
        assert_eq!(normalize_world_query("MAIN"), None);
        assert_eq!(normalize_world_query(""), None);
    }

    #[test]
    fn invalid_lot_in_the_address_bar_is_kept_instead_of_widening_the_bid_log() {
        // 超长或含控制字符的 lot 原样保留，竞价日志页据此报字段错误且不发请求。
        let long_lot = "a".repeat(129);
        let query = parse_public_query("/bidlog", &format!("?lot={long_lot}"));
        assert_eq!(query.bid_log_lot.as_deref(), Some(long_lot.as_str()));
        let query = parse_public_query("/bidlog", "?lot=One%0712");
        assert_eq!(query.bid_log_lot.as_deref(), Some("One\u{7}12"));
        // 只去两端空白，空值仍表示不筛选；历史名称沿用同一口径。
        let query = parse_public_query("/bidlog", "?lot=%20One12%20");
        assert_eq!(query.bid_log_lot.as_deref(), Some("One12"));
        assert_eq!(parse_public_query("/bidlog", "?lot=%20%09").bid_log_lot, None);
        let query = parse_public_query("/history", "?name=+Phase+Robe+");
        assert_eq!(query.history_name.as_deref(), Some("Phase Robe"));
    }

    #[test]
    fn linked_page_sizes_above_the_public_maximum_restore_the_default() {
        // 旧链接里超过契约上限的数量按非法值恢复默认，不原样发给必然拒绝它的接口。
        let query = parse_public_query("/", "?page_size=200&movement_page_size=200");
        assert_eq!((query.page_size, query.movement_page_size), (200, 200));
        let query = parse_public_query("/", "?page_size=201&movement_page_size=1000");
        assert_eq!((query.page_size, query.movement_page_size), (20, 20));
        let query = parse_public_query("/compensation", "?page_size=0&movement_page_size=-5");
        assert_eq!((query.page_size, query.movement_page_size), (20, 20));
    }

    #[test]
    fn passthrough_parameters_survive_round_trip_encode() {
        let query = PublicBoardQuery {
            passthrough: vec![("ref".to_owned(), "a&b=c".to_owned())],
            ..PublicBoardQuery::default()
        };
        let encoded = public_query_string(&query);
        assert!(encoded.contains("ref=a%26b%3Dc"));

        // 页面拥有的键排在外部透传键之前，外部键不覆盖 canonical 状态。
        let keys = public_query_pairs(&query);
        let page_size = keys.iter().position(|(key, _)| key == "page_size").unwrap();
        let passthrough = keys.iter().position(|(key, _)| key == "ref").unwrap();
        assert!(page_size < passthrough);
    }

    #[test]
    fn auction_query_distinguishes_absence_valid_ids_and_invalid_values() {
        const AUCTION: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let absent = parse_public_query("/", "");
        assert_eq!(absent.auction_id(), None);
        assert_eq!(absent.auction_parameter(), &AuctionParameter::Unspecified);
        assert!(!public_query_pairs(&absent).iter().any(|(key, _)| key == "auction"));

        for version in 1..=5 {
            let auction = AUCTION.replace("426f", &format!("{version}26f"));
            let valid = parse_public_query("/", &format!("?auction={auction}"));
            assert_eq!(valid.auction_id(), Some(auction.as_str()));
            assert_eq!(valid.auction_parameter(), &AuctionParameter::Selected(auction));
        }

        for value in [
            String::new(),
            "truncated".to_owned(),
            AUCTION.to_uppercase(),
            AUCTION.replace("426f", "726f"),
            AUCTION.replace("9247", "4247"),
            AUCTION.replace('-', ""),
            format!("{{{AUCTION}}}"),
            format!("urn:uuid:{AUCTION}"),
        ] {
            let invalid = parse_public_query("/", &format!("?auction={value}"));
            assert_eq!(invalid.auction_id(), None, "非法原文不能作为读取目标投影");
            assert_eq!(
                invalid.auction_parameter(),
                &AuctionParameter::Invalid(vec![value.clone()])
            );
            assert!(public_query_pairs(&invalid).contains(&("auction".to_owned(), value)));
        }

        let duplicate = parse_public_query("/", &format!("?auction={AUCTION}&auction={AUCTION}"));
        assert!(matches!(duplicate.auction_parameter(), AuctionParameter::Invalid(_)));
        assert_eq!(
            public_query_pairs(&duplicate).into_iter().filter(|(key, _)| key == "auction").count(),
            2
        );
    }

    #[test]
    fn auction_link_selection_and_clear_replace_the_invalid_values() {
        let id = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let invalid = parse_public_query("/", "?auction=&auction=broken&world=isekai");
        assert_eq!(
            invalid.auction_parameter(),
            &AuctionParameter::Invalid(vec![String::new(), "broken".to_owned()])
        );
        for selection in [Some(id.to_owned()), None] {
            let href = public_href_for_auction("/", invalid.clone(), selection.clone());
            let parsed = parse_public_query("/", location_parts(&href).1);
            assert_eq!(parsed.world.as_deref(), Some("isekai"));
            assert_eq!(parsed.auction_id(), selection.as_deref());
            assert!(!matches!(parsed.auction_parameter(), AuctionParameter::Invalid(_)));
        }
    }
}

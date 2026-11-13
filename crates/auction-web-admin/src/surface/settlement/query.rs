//! Admin 结算报表筛选的 URL 状态与浏览器历史同步。
//! 为什么不迁到统一的 `use_query_state`：本页的 URL 状态被拆成四个独立 signal
//! （world/auction/sequence/page），popstate 恢复不是「回写单个状态信号」而是经
//! `use_settlement_location_revision` 的 revision 驱动页面 effect 一次性恢复四个 signal，
//! 并同时复位详情上下文（`selection_generation` 防止 recovery 确认异步回写写到已切换的
//! 选择上，见 recovery.rs 提交回调里的同代比较）；目录加载完成后还要按服务端事实把地址
//! canonical 化（auction 与 sequence 必须成对、空目录页钳制到末页、解析失败保留地址显式
//! 报错）。这些「读-改-写 + 跨 signal 复位 + 服务端事实纠偏」语义超出通用 hook 的单一状态
//! 模型，整体迁移会重写整页 signal 管线并需要重新验证异步守卫，因此保留本模块的三件套
//! API（read + push/replace + location revision），底层地址栏读写与 popstate 监听复用
//! `auction_web_shared::query` 的 `read_query_pairs` / `write_query_pairs` / `use_popstate_revision`。

use dioxus::prelude::*;
use uuid::Uuid;

use auction_web_shared::{
    QueryWriteMode, read_query_pairs, use_popstate_revision, write_query_pairs,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettlementQuery {
    pub world_id: String,
    pub auction_id: String,
    pub sequence: Option<i64>,
    pub page: i64,
}

impl Default for SettlementQuery {
    fn default() -> Self {
        Self { world_id: "all".to_owned(), auction_id: String::new(), sequence: None, page: 1 }
    }
}

/// 本页在地址栏管理的全部 query 键；写回时只替换这些键，其余键原样保留。
const SETTLEMENT_QUERY_KEYS: &[&str] = &["world", "auction", "sequence", "page"];

pub(super) fn read_settlement_query() -> SettlementQuery {
    parse_settlement_pairs(&read_query_pairs())
}

fn parse_settlement_pairs(pairs: &[(String, String)]) -> SettlementQuery {
    let mut query = SettlementQuery::default();
    for (key, value) in pairs {
        match key.as_str() {
            "world" if matches!(value.as_str(), "main" | "isekai") => {
                query.world_id = value.clone();
            }
            "auction" => {
                if let Ok(id) = Uuid::parse_str(value)
                    && !id.is_nil()
                    && id.to_string() == *value
                {
                    query.auction_id = value.clone();
                }
            }
            "sequence" => {
                if let Ok(sequence) = value.parse::<i64>()
                    && sequence > 0
                {
                    query.sequence = Some(sequence);
                }
            }
            "page" => {
                if let Ok(page) = value.parse::<i64>()
                    && page > 0
                {
                    query.page = page;
                }
            }
            _ => {}
        }
    }
    // UUID 不能独立定位其所在目录页；canonical 地址始终把 auction 与 sequence 成对写入。
    if query.sequence.is_none() {
        query.auction_id.clear();
    }
    query
}

pub(super) fn replace_settlement_query(query: &SettlementQuery) {
    write_query_pairs(
        SETTLEMENT_QUERY_KEYS,
        &settlement_query_pairs(query),
        QueryWriteMode::Replace,
    );
}

/// 用户主动切换筛选或分页时创建可返回的浏览器历史项。
pub(super) fn push_settlement_query(query: &SettlementQuery) {
    write_query_pairs(SETTLEMENT_QUERY_KEYS, &settlement_query_pairs(query), QueryWriteMode::Push);
}

/// 序列化规则与旧版一致：默认值键省略，auction 与 sequence 成对写入。
fn settlement_query_pairs(query: &SettlementQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if query.world_id != "all" {
        pairs.push(("world".to_owned(), query.world_id.clone()));
    }
    if let Some(sequence) = query.sequence {
        pairs.push(("sequence".to_owned(), sequence.to_string()));
    }
    if !query.auction_id.is_empty() {
        pairs.push(("auction".to_owned(), query.auction_id.clone()));
    }
    if query.page > 1 {
        pairs.push(("page".to_owned(), query.page.to_string()));
    }
    pairs
}

/// 监听同一路径的浏览器返回/前进，确保筛选状态不会与地址栏分离。
pub(super) fn use_settlement_location_revision() -> Signal<u64> {
    let revision = use_signal(|| 0_u64);
    use_popstate_revision(revision);
    revision
}

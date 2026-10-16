//! Public 列表的统一分页选项、译文和查询状态更新；不改变 Admin 分页策略。

use auction_web_shared::{Pager, QueryWriteMode, legacy_t as t};
use dioxus::prelude::*;

use super::{PublicBoardQuery, read_public_query, update_public_query_state};

mod preferences;

#[cfg(target_arch = "wasm32")]
pub(crate) use preferences::parse_public_page_size;
pub(crate) use preferences::stored_public_page_size;

pub(crate) const DEFAULT_PAGE_SIZE: i64 = 10;
pub(crate) const PUBLIC_PAGE_SIZES: [i64; 4] = [10, 20, 50, 100];

/// 只接受可表示的正整数；非法数量回退默认值，不静默截断合法的大数量。
pub(crate) fn normalize_public_page_size(size: i64) -> i64 {
    if size > 0 { size } else { DEFAULT_PAGE_SIZE }
}

/// 切换数量与回到首页作为一次 URL 更新，保留世界、场次和筛选条件。
pub(crate) fn update_public_page_size(state: Signal<PublicBoardQuery>, size: i64) {
    update_public_query_state(
        state,
        |query| {
            query.page_size = normalize_public_page_size(size);
            query.page = 1;
        },
        QueryWriteMode::Push,
    );
}

/// 资源重读期间不能把旧页总数用于新分页状态的纠偏；可选字段沿用旧 DTO 兼容语义。
pub(crate) fn public_page_matches(
    page: impl Into<Option<i64>>,
    page_size: impl Into<Option<i64>>,
    requested_page: i64,
    requested_size: i64,
) -> bool {
    page.into().is_none_or(|page| page == requested_page)
        && page_size.into().is_none_or(|size| size == requested_size)
}

/// 注入 Public 五语言文案、正整数数量与本地偏好，分页计算及键盘交互由共享组件负责。
#[component]
pub(crate) fn PublicPager(
    page: i64,
    page_size: i64,
    total: Option<i64>,
    #[props(default)] has_more: bool,
    /// 补偿流水使用独立偏好，不覆盖主列表的每页数量。
    #[props(default)]
    movement: bool,
    /// 大多数 Public API 的 offset 上限为 10000，黑名单单独注入其既有边界。
    #[props(default = 10_000)]
    max_offset: i64,
    label: String,
    on_page_change: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
) -> Element {
    rsx! {
        Pager {
            page,
            page_size,
            total,
            has_more,
            max_page: max_offset / page_size.max(1) + 1,
            label,
            previous_label: t!("catalog-previous"),
            status: move |(page, pages)| t!("catalog-page", page: page, pages: pages),
            next_label: t!("catalog-next"),
            jump_label: t!("pagination-jump"),
            jump_button_label: t!("catalog-page-go"),
            total_summary: total.map(|count| t!("pagination-total", count: count)).unwrap_or_default(),
            page_size_options: PUBLIC_PAGE_SIZES.to_vec(),
            page_size_label: t!("pagination-size"),
            // 数量用字符串插值，避免 Fluent 浮点格式化舍入较大的合法 i64。
            page_size_option_label: Callback::new(move |count: i64| t!("pagination-size-option", count: count.to_string())),
            allow_custom_page_size: true,
            page_size_custom_label: t!("pagination-custom"),
            page_size_apply_label: t!("pagination-apply"),
            page_size_invalid_label: t!("pagination-positive-integer"),
            on_page_change,
            on_page_size_change: move |size| {
                if size <= 0 {
                    return;
                }
                // 先冻结离开条目的数量（含 10），再更新 URL 和偏好；返回时不读到新偏好。
                // 只替换数量键，不能覆盖出价日志尚在防抖写回的筛选。
                let previous = read_public_query();
                auction_web_shared::write_query_pairs(
                    &["page_size", "movement_page_size"],
                    &[
                        ("page_size".to_owned(), previous.page_size.to_string()),
                        ("movement_page_size".to_owned(), previous.movement_page_size.to_string()),
                    ],
                    QueryWriteMode::Replace,
                );
                on_page_size_change.call(size);
                preferences::save_public_page_size(size, movement);
            },
        }
    }
}

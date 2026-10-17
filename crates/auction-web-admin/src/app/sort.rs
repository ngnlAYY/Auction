//! Admin 列表表格的表头排序共享实现。
//! 服务端 admin 列表端点不接受排序参数（`api/` 各列表请求与 `auction-contracts` DTO 均无
//! sort/order 字段，服务端按固定顺序返回当前页），因此排序只在浏览器内对当前页数据执行；
//! 表头 tooltip 明确「仅当前页」局限。排序状态经 `query.rs` 与筛选/分页一起入 URL，
//! 刷新、分享链接与浏览器前进/后退都能恢复。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

/// 排序方向。地址栏 `dir` 只区分 `desc` 与其余值（含缺省），非法值一律回退升序。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SortDirection {
    #[default]
    Asc,
    Desc,
}

impl SortDirection {
    /// 解析地址栏 `dir` 值；仅 `desc` 视为降序。
    pub(super) fn parse(value: Option<&str>) -> Self {
        match value {
            Some("desc") => Self::Desc,
            _ => Self::Asc,
        }
    }

    /// 序列化为地址栏值；升序是默认值，返回 None 表示省略该键，保持地址栏干净。
    pub(super) fn as_query_value(self) -> Option<&'static str> {
        match self {
            Self::Asc => None,
            Self::Desc => Some("desc"),
        }
    }

    /// 点击已排序列时切换方向。
    pub(super) const fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }

    /// `aria-sort` 的取值（仅当前排序列使用）。
    const fn aria(self) -> &'static str {
        match self {
            Self::Asc => "ascending",
            Self::Desc => "descending",
        }
    }

    /// 表头的方向箭头，仅作视觉指示，对辅助技术隐藏。
    const fn arrow(self) -> &'static str {
        match self {
            Self::Asc => "▲",
            Self::Desc => "▼",
        }
    }
}

/// 表头点击后的下一排序状态：同列切换方向，切到另一列时从升序开始。
pub(super) fn next_sort(
    active: &str,
    direction: SortDirection,
    column: &str,
) -> (String, SortDirection) {
    if active == column {
        (column.to_owned(), direction.toggled())
    } else {
        (column.to_owned(), SortDirection::Asc)
    }
}

/// 按排序方向应用比较结果：升序保持、降序反转。配合稳定的 `sort_by` 保证等值行顺序确定。
pub(super) fn directed(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Asc => ordering,
        SortDirection::Desc => ordering.reverse(),
    }
}

/// 可排序表头单元格。
/// `aria-sort` 始终给出当前状态（未排序列为 `none`），点击按钮切换排序；
/// 按钮 title 提示排序仅对当前页生效。排序只改变当前页的展示顺序，
/// 不改变请求参数，也不跨页重排。
#[component]
pub(super) fn SortableTh(
    /// 本列的稳定列标识（与 query.rs 中该页允许集合一致）。
    column: &'static str,
    label: String,
    /// 当前排序列；空串表示保持服务端原始顺序。
    active: String,
    /// 当前排序方向（仅 active 为本列时有意义）。
    direction: SortDirection,
    /// 点击后回调下一排序状态（列标识 + 方向）。
    on_sort: EventHandler<(String, SortDirection)>,
    /// 附加在 th 上的样式类。
    #[props(default)]
    class: Option<&'static str>,
) -> Element {
    let is_active = active == column;
    let aria_sort = if is_active { direction.aria() } else { "none" };
    let hint = if is_active {
        match direction {
            SortDirection::Asc => t!("admin-sort-ascending-hint"),
            SortDirection::Desc => t!("admin-sort-descending-hint"),
        }
    } else {
        t!("admin-sort-inactive-hint")
    };
    rsx! {
        th {
            scope: "col",
            class: class.unwrap_or(""),
            aria_sort,
            button {
                r#type: "button",
                class: "admin-sort-trigger",
                title: hint,
                onclick: move |_| {
                    on_sort.call(next_sort(&active, direction, column));
                },
                {label}
                span { class: "admin-sort-arrow", aria_hidden: "true",
                    {if is_active { direction.arrow() } else { "↕" }}
                }
            }
        }
    }
}

//! Admin 分页 owner：数量 URL、独立本地偏好、共享控件与请求身份门禁。

use std::{future::Future, rc::Rc};

use auction_web_shared::{
    Pager, QueryState, QueryWriteMode, legacy_t as t, use_query_state, write_query_pairs,
};
use dioxus::prelude::*;

mod preferences;

/// 同页各表分别持有数量信号；数量不进入 API helper 的浏览器依赖。
#[derive(Clone)]
pub(crate) struct AdminPagination {
    query: QueryState<i64>,
    scope: &'static str,
    keys: &'static [&'static str],
    maximum: Option<i64>,
    max_offset: i64,
}

impl PartialEq for AdminPagination {
    fn eq(&self, other: &Self) -> bool {
        self.query.state == other.query.state && self.scope == other.scope
    }
}

impl AdminPagination {
    /// 手工多信号页面显式订阅数量，保留其既有 URL 同步协议。
    pub(crate) fn size_signal(&self) -> Signal<i64> {
        self.query.state
    }

    pub(crate) fn size(&self) -> i64 {
        (self.query.state)()
    }
}

/// URL 明确值（含非法回退 10）优先；浏览链接不保存偏好。
pub(crate) fn use_admin_pagination(
    scope: &'static str,
    keys: &'static [&'static str],
    maximum: Option<i64>,
    max_offset: i64,
) -> AdminPagination {
    let query = use_query_state(
        keys,
        move |pairs| {
            pairs.iter().find(|(key, _)| key == keys[0]).map_or_else(
                || preferences::load(scope, maximum),
                |(_, value)| preferences::parse(value, maximum).unwrap_or(10),
            )
        },
        move |size| vec![(keys[0].to_owned(), size.to_string())],
    );
    let size = query.state;
    // 初次访问也固定有效数量，之后筛选/排序/翻页生成的每条历史都会继承它。
    // 仅规范 URL，不保存偏好；返回旧条目不会被后来保存的偏好污染。
    use_effect(move || {
        write_query_pairs(
            keys,
            &[(keys[0].to_owned(), size().to_string())],
            QueryWriteMode::Replace,
        );
    });
    AdminPagination { query, scope, keys, maximum, max_offset }
}

/// 旧成功结果只在查询身份及数量都一致时可见；刷新同一请求不清除未完成命令。
/// 请求闭包可订阅 reload signal，但返回身份应仅含筛选、实体与页码。
pub(crate) fn use_admin_page<Q, T, F>(
    pagination: AdminPagination,
    request: impl Fn() -> Q + 'static,
    fetch: impl Fn(Q, i64) -> F + 'static,
) -> Memo<Option<Result<T, String>>>
where
    Q: Clone + PartialEq + 'static,
    T: Clone + PartialEq + 'static,
    F: Future<Output = Result<T, String>> + 'static,
{
    use_admin_page_with_state(pagination, request, fetch).0
}

/// 额外返回读取状态，供自动轮询跳过仍在进行的请求；展示身份与快照复用规则保持一致。
pub(crate) fn use_admin_page_with_state<Q, T, F>(
    pagination: AdminPagination,
    request: impl Fn() -> Q + 'static,
    fetch: impl Fn(Q, i64) -> F + 'static,
) -> (Memo<Option<Result<T, String>>>, ReadSignal<UseResourceState>)
where
    Q: Clone + PartialEq + 'static,
    T: Clone + PartialEq + 'static,
    F: Future<Output = Result<T, String>> + 'static,
{
    let request = use_hook(|| Rc::new(request));
    let fetch = use_hook(|| Rc::new(fetch));
    let resource = use_resource({
        let request = request.clone();
        let pagination = pagination.clone();
        move || {
            let identity = request();
            let size = pagination.size();
            let future = fetch(identity.clone(), size);
            async move { (identity, size, future.await) }
        }
    });
    // reload 只驱动请求；同一身份开始重读时不再复制并深比较整页旧结果。
    let requested = use_memo(move || (request(), pagination.size()));
    let result = use_memo(move || {
        let requested = requested.read();
        let (identity, size) = &*requested;
        resource
            .read()
            .as_ref()
            .filter(|(loaded, loaded_size, _)| loaded == identity && loaded_size == size)
            .map(|(_, _, result)| result.clone())
    });
    (result, resource.state())
}

/// 保留表格原有标签，统一注入翻页/跳页文案、数字页码、数量文案、上限与偏好写入。
#[component]
pub(crate) fn AdminPager(
    pagination: AdminPagination,
    page: i64,
    page_size: i64,
    total: Option<i64>,
    #[props(default)] has_more: bool,
    label: String,
    on_page_change: EventHandler<i64>,
    /// 本地报告页码不写历史；数量选择自行创建历史项。
    #[props(default)]
    local: bool,
    /// 目录数量变化只重置目录页，不清除其他独立报告分页或命令上下文。
    #[props(default)]
    on_size_reset: Option<EventHandler<()>>,
) -> Element {
    let max_page =
        pagination.max_offset.checked_div(page_size.max(1)).unwrap_or(0).saturating_add(1);
    let maximum = pagination.maximum;
    let current_size = pagination.size_signal();
    rsx! {
        Pager {
            page, page_size, total, has_more,
            max_page,
            max_page_size: maximum,
            label,
            previous_label: t!("admin-pagination-previous"),
            status: Callback::new(|(page, pages): (i64, i64)| t!("admin-pagination-status", page: page, pages: pages)),
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            total_summary: total.map(|count| t!("admin-pagination-total", count: count.to_string())).unwrap_or_default(),
            range_summary: Some(Callback::new(|(start, end, total_count): (i64, i64, i64)| {
                t!(
                    "admin-pagination-range",
                    start: start.to_string(),
                    end: end.to_string(),
                    total: total_count.to_string()
                )
            })),
            page_size_options: vec![10, 20, 50, 100],
            page_size_label: t!("admin-pagination-size"),
            page_size_option_label: Callback::new(move |count: i64| t!("admin-pagination-size-option", count: count.to_string())),
            allow_custom_page_size: true,
            page_size_custom_label: t!("admin-pagination-custom"),
            page_size_apply_label: t!("admin-pagination-apply"),
            page_size_invalid_label: maximum.map_or_else(
                || t!("admin-pagination-positive-integer"),
                |maximum| t!("admin-pagination-size-limit", maximum: maximum.to_string()),
            ),
            on_page_change: move |next| {
                // 数量已切换时，旧控件尚未卸载的纠偏不能覆盖新请求页码。
                if current_size() == page_size { on_page_change.call(next); }
            },
            on_page_size_change: move |size| {
                if current_size() != page_size || size <= 0 || maximum.is_some_and(|maximum| size > maximum) { return; }
                // 先固定离开历史项的旧数量，随后由原页面 push 首页，再 replace 新数量。
                // 各 hook 只写自己拥有的键，保留筛选、排序及未知参数。
                write_query_pairs(pagination.keys, &[(pagination.keys[0].to_owned(), pagination.size().to_string())], QueryWriteMode::Replace);
                if let Some(reset) = on_size_reset {
                    reset.call(());
                } else {
                    on_page_change.call(1);
                }
                let mut query = pagination.query.clone();
                if local { query.push(size); } else { query.replace(size); }
                preferences::save(pagination.scope, size);
            },
        }
    }
}

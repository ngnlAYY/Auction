//! Public 列表的统一分页选项、译文和查询状态更新；不改变 Admin 分页策略。
//! 回调属性须为非可选类型，避免更新时拷入会随临时 VNode 释放的句柄
//! （Dioxus 0.7.10 的 `dioxus-core-macro/src/props/mod.rs:694–715` 与 `dioxus-core/src/events.rs:555–556`）。

use auction_contracts::MAX_PUBLIC_PAGE_SIZE;
use auction_web_shared::{Pager, QueryWriteMode, t};
use dioxus::prelude::*;

use super::PublicBoardQuery;

mod bounds;
mod location;
mod preferences;
#[cfg(test)]
mod test_support;

pub(crate) use bounds::{
    PageKey, PublicPageClampError, public_page_within_offset, use_public_page_bounds,
};
pub(crate) use location::{
    commit_public_query_pairs, page_pathname, query_values, read_page_pairs, read_page_query,
    use_page_query, use_public_query_pairs, write_page_pairs,
};
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use preferences::parse_public_page_size;
pub(crate) use preferences::stored_public_page_size;
#[cfg(test)]
pub(crate) use test_support::{
    PageBrowserFixture, PageReadFixture, assert_selected_error, dispatch_page_event,
    page_event_target, page_test_contexts, rendered_event_target, rendered_role_contains,
    rendered_select_option, run_page_events, settle_page,
};

use self::bounds::{MAX_PUBLIC_PAGE_OFFSET, offset_last_page};

pub(crate) const DEFAULT_PAGE_SIZE: i64 = 20;
pub(crate) const PUBLIC_PAGE_SIZES: [i64; 4] = [10, 20, 50, 100];

// 预设与默认值都必须是后端接受的页长，否则选择预设就会得到 400。
const _: () = assert!(DEFAULT_PAGE_SIZE >= 1 && DEFAULT_PAGE_SIZE <= MAX_PUBLIC_PAGE_SIZE);
const _: () = assert!(PUBLIC_PAGE_SIZES[3] <= MAX_PUBLIC_PAGE_SIZE);

/// 每页数量只接受 1 到契约上限 [`MAX_PUBLIC_PAGE_SIZE`]：超出的页长后端一律拒绝，不能写进 URL、
/// 偏好或请求。
pub(crate) fn accepted_public_page_size(size: i64) -> Option<i64> {
    (1..=MAX_PUBLIC_PAGE_SIZE).contains(&size).then_some(size)
}

/// 非法或超过上限的数量回退默认值，不截断成上限：截断会把用户没有选过的数量写进 URL。
pub(crate) fn normalize_public_page_size(size: i64) -> i64 {
    accepted_public_page_size(size).unwrap_or(DEFAULT_PAGE_SIZE)
}

/// 切换数量与回到首页作为一次 URL 更新，保留世界、场次和筛选条件。
pub(crate) fn update_public_page_size(state: Signal<PublicBoardQuery>, size: i64) {
    set_public_page_size(state, size);
}

/// 地址确认后同时提交页长与首页；只写分页键，非法筛选仍留在原地址。
pub(crate) fn set_public_page_size(state: Signal<PublicBoardQuery>, size: i64) -> bool {
    let keys = if page_pathname() == "/bidlog" {
        &["page_size", "page", "bidlog_page"][..]
    } else {
        &["page_size", "page"][..]
    };
    commit_public_query_pairs(
        state,
        keys,
        &[("page_size".to_owned(), normalize_public_page_size(size).to_string())],
        QueryWriteMode::Push,
    )
}

fn commit_page_size_choice(
    freeze: impl FnOnce() -> bool,
    write: impl FnOnce() -> bool,
    save: impl FnOnce(),
) -> bool {
    // 冻结旧历史项以后才提交新的数量。任何一次拒写都不能改变本地偏好。
    if !freeze() || !write() {
        return false;
    }
    save();
    true
}

fn freeze_page_sizes(applied: &[(String, String)]) -> bool {
    let previous = read_page_query();
    let missing: Vec<_> =
        [("page_size", previous.page_size), ("movement_page_size", previous.movement_page_size)]
            .into_iter()
            .filter(|(key, _)| !applied.iter().any(|(name, _)| name == key))
            .collect();
    if missing.is_empty() {
        return true;
    }
    let keys: Vec<_> = missing.iter().map(|(key, _)| *key).collect();
    let pairs: Vec<_> =
        missing.iter().map(|(key, size)| ((*key).to_owned(), size.to_string())).collect();
    write_page_pairs(&keys, &pairs, QueryWriteMode::Replace)
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

/// 注入 Public 五语言文案、1 到契约上限的数量与本地偏好，分页计算及键盘交互由共享组件负责。
/// `hide_when_single` / `show_size_picker` 覆盖账号页与装备属性表的经典布局，默认仍是
/// 目录的数字页码加每页数量选择器。
#[component]
pub(crate) fn PublicPager(
    page: i64,
    page_size: i64,
    total: Option<i64>,
    #[props(default)] has_more: bool,
    /// 补偿流水使用独立偏好，不覆盖主列表的每页数量。
    #[props(default)]
    movement: bool,
    /// 大多数 Public API 的 offset 上限为 10000，黑名单单独注入其既有边界；
    /// `None` 表示该列表按后端总数分页，不受 offset 限制。
    #[props(default = Some(MAX_PUBLIC_PAGE_OFFSET))]
    max_offset: Option<i64>,
    /// 单页时保留挂载但隐藏控件，让总量收缩后的页码与 URL 仍能自动收敛。
    #[props(default)]
    hide_when_single: bool,
    /// 关闭数字页码与每页数量选择器，保留经典上一页/下一页布局。
    #[props(default = true)]
    show_size_picker: bool,
    /// 覆盖跳页输入文案；默认使用 Public 通用 `pagination-jump`。
    #[props(default)]
    jump_label: Option<String>,
    /// 接口自带更小的每页数量上限时注入（如履约进度为 100）：超出的预设禁用，自定义输入就地提示
    /// “1–上限”的整数范围。缺省即契约上限 [`MAX_PUBLIC_PAGE_SIZE`]（200），注入值不能放宽它。
    #[props(default)]
    max_page_size: Option<i64>,
    label: String,
    on_page_change: Callback<i64, bool>,
    #[props(default)] on_page_clamp: Callback<i64, bool>,
    #[props(default)] on_page_size_change: EventHandler<i64>,
    #[props(default)] on_page_size_commit: Callback<i64, Option<bool>>,
    /// 指定了返回地址写入结果的纠偏回调时启用原地重试。
    #[props(default)]
    report_clamp_failure: bool,
) -> Element {
    let page_size = page_size.max(1);
    let pathname = page_pathname();
    let maximum =
        max_page_size.map_or(MAX_PUBLIC_PAGE_SIZE, |limit| limit.min(MAX_PUBLIC_PAGE_SIZE));
    let pager = rsx! {
        Pager {
            page,
            page_size,
            total,
            has_more,
            max_page: max_offset.map(|max_offset| offset_last_page(page_size, max_offset)),
            label,
            previous_label: t!("catalog-previous"),
            status: move |(page, pages)| t!("catalog-page", page: page, pages: pages),
            next_label: t!("catalog-next"),
            jump_label: jump_label.unwrap_or_else(|| t!("pagination-jump")),
            jump_button_label: t!("catalog-page-go"),
            total_summary: total.map_or_default(|count| t!("pagination-total", count: count)),
            show_range_summary: true,
            range_summary: move |(start, end, total_count): (i64, i64, i64)| {
                t!(
                    "pagination-range",
                    start: start.to_string(),
                    end: end.to_string(),
                    total: total_count.to_string()
                )
            },
            page_size_options: if show_size_picker { PUBLIC_PAGE_SIZES.to_vec() } else { Vec::new() },
            page_size_label: t!("pagination-size"),
            // 数量用字符串插值，避免 Fluent 浮点格式化舍入较大的合法 i64。
            page_size_option_label: move |count: i64| t!("pagination-size-option", count: count.to_string()),
            // 自定义输入超过上限时由共享组件提示错误且不提交，不会发出必然 400 的请求。
            max_page_size: Some(maximum),
            allow_custom_page_size: true,
            page_size_custom_label: t!("pagination-custom"),
            page_size_apply_label: t!("pagination-apply"),
            page_size_invalid_label: t!("pagination-size-limit", maximum: maximum.to_string()),
            on_page_change: move |page| { on_page_change.call(page); },
            on_page_commit: move |page| Some(on_page_change.call(page)),
            on_page_clamp: move |page| on_page_clamp.call(page),
            clamp_failure: report_clamp_failure.then(|| (t!("pagination-clamp-error"), t!("data-retry"))),
            on_page_size_commit: move |size| Some({
                // 共享分页器已拒绝超出上限的数量；这里再守一次，不把它写进地址栏与偏好。
                if accepted_public_page_size(size).is_none_or(|size| size > maximum) {
                    return Some(false);
                }
                commit_page_size_choice(
                    || page_pathname() == pathname && freeze_page_sizes(&read_page_pairs()),
                    || on_page_size_commit.call(size).unwrap_or_else(|| {
                        let before = read_page_pairs();
                        on_page_size_change.call(size);
                        let after = read_page_pairs();
                        let key = if movement { "movement_page_size" } else { "page_size" };
                        // 尚未返回写入结果的调用方，须由实际地址确认本次提交。
                        before != after && after.iter().any(|(name, value)| name == key && *value == size.to_string())
                    }),
                    || preferences::save_public_page_size(size, movement),
                )
            }),
        }
    };
    if hide_when_single {
        let single_page =
            (!report_clamp_failure || page <= 1) && total.is_some_and(|total| total <= page_size);
        return rsx! {
            div { hidden: single_page, {pager} }
        };
    }
    pager
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use auction_contracts::MAX_PUBLIC_PAGE_SIZE;
    use auction_web_shared::QueryWriteMode;
    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use dioxus::dioxus_core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{
        DEFAULT_PAGE_SIZE, PUBLIC_PAGE_SIZES, PageKey, PublicPager, PublicPagerProps,
        accepted_public_page_size, commit_page_size_choice, normalize_public_page_size,
        parse_public_page_size, set_public_page_size, use_page_query,
    };
    use crate::surface::PageBrowserFixture;
    use crate::surface::router::PUBLIC_LOCALE_CATALOG;

    #[derive(Default)]
    struct ClampFixture {
        revision: Cell<i64>,
        calls: RefCell<Vec<(i64, i64)>>,
    }

    fn init_english() {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                PUBLIC_LOCALE_CATALOG.default_ftl,
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        })
        .expect("加载公开端英文译文");
    }

    fn clamp_fixture() -> Element {
        init_english();
        let fixture = consume_context::<Rc<ClampFixture>>();
        let revision = fixture.revision.get();
        let fallback = fixture.clone();
        let props = PublicPagerProps::builder()
            .page(if matches!(revision, 1 | 2 | 5) { 2 } else { 10 + revision })
            .page_size(10)
            .total(Some(40))
            .label("翻页".to_owned())
            .on_page_change(move |page| fallback.calls.borrow_mut().push((-revision, page)));
        let props = if (1..=3).contains(&revision) {
            props
                .on_page_clamp(move |page| fixture.calls.borrow_mut().push((revision, page)))
                .build()
        } else {
            props.build()
        };
        rsx! { PublicPager { ..props } }
    }

    #[test]
    fn public_clamp_uses_the_latest_callback_and_defaults_to_page_change() {
        let fixture = Rc::new(ClampFixture::default());
        let mut dom = VirtualDom::new(clamp_fixture).with_root_context(fixture.clone());
        dom.rebuild_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(&*fixture.calls.borrow(), &[(0, 4)]);
        for revision in 1..=5 {
            fixture.revision.set(revision);
            dom.mark_dirty(ScopeId::APP);
            dom.render_immediate_to_vec();
            dom.render_immediate_to_vec();
        }
        assert_eq!(&*fixture.calls.borrow(), &[(0, 4), (3, 4), (-4, 4)]);
    }

    fn offset_fixture() -> Element {
        init_english();
        rsx! {
            PublicPager {
                page: 1001, page_size: 10, total: Some(10_020),
                label: "分页".to_owned(), on_page_change: |_| {},
            }
        }
    }

    #[test]
    fn numbered_pager_includes_page_1001_but_never_offers_page_1002() {
        let mut dom = VirtualDom::new(offset_fixture);
        let initial = dom.rebuild_to_vec();
        assert!(initial.edits.iter().any(|edit| matches!(
            edit, Mutation::SetAttribute { name: "max", value: AttributeValue::Text(value), .. }
                if value == "1001"
        )));
        assert!(initial.edits.iter().any(|edit| matches!(
            edit, Mutation::CreateTextNode { value, .. } if value == "1001"
        )));
        assert!(!initial.edits.iter().any(|edit| matches!(
            edit, Mutation::CreateTextNode { value, .. } if value == "1002"
        )));
    }

    #[test]
    fn page_size_choices_stop_at_the_contract_maximum() {
        assert_eq!(MAX_PUBLIC_PAGE_SIZE, 200);
        for size in [1, 37, 200].into_iter().chain(PUBLIC_PAGE_SIZES) {
            assert_eq!(accepted_public_page_size(size), Some(size), "{size}");
            assert_eq!(normalize_public_page_size(size), size, "{size}");
        }
        // 自定义输入 201 等越界数量不被接受：分页器不写 URL、不保存偏好，也就不会发出请求。
        for size in [201, 1_000, i64::MAX, 0, -1, i64::MIN] {
            assert_eq!(accepted_public_page_size(size), None, "{size}");
            // 其他入口拿到越界值时回退默认值，不截断成上限。
            assert_eq!(normalize_public_page_size(size), DEFAULT_PAGE_SIZE, "{size}");
        }
    }

    #[test]
    fn stored_and_linked_page_sizes_above_the_maximum_are_invalid() {
        // URL 与本地偏好共用同一解析：超过上限的旧链接或旧偏好按非法值处理，由调用方恢复默认值。
        assert_eq!(parse_public_page_size("200"), Some(200));
        assert_eq!(parse_public_page_size("1"), Some(1));
        for value in ["201", "500", "9223372036854775807", "99999999999999999999"] {
            assert_eq!(parse_public_page_size(value), None, "{value}");
        }
        for value in ["", "0", "+20", "-20", "20.0", "2e1", " 20", "20 "] {
            assert_eq!(parse_public_page_size(value), None, "{value}");
        }
    }

    #[test]
    fn preference_follows_address_commit_and_a_failed_freeze_blocks_the_push() {
        let preference = Cell::new(20);
        let attempts = Cell::new(0);
        for (freeze, push, saved, attempt_count) in
            [(false, false, 20, 0), (true, false, 20, 1), (true, true, 50, 2)]
        {
            assert_eq!(
                commit_page_size_choice(
                    || freeze,
                    || {
                        attempts.set(attempts.get() + 1);
                        push
                    },
                    || preference.set(50)
                ),
                push
            );
            assert_eq!(preference.get(), saved);
            assert_eq!(attempts.get(), attempt_count);
        }
    }

    #[test]
    fn accepted_page_size_and_page_push_both_reach_the_live_query_signal() {
        fn mounted_query() -> Element {
            use_page_query();
            rsx! {}
        }

        let browser = PageBrowserFixture::new("/bidlog", "bidlog_page=2&lot=One12");
        let mut dom = VirtualDom::new(mounted_query).with_root_context(browser.clone());
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            let state = browser.signal();
            assert!(set_public_page_size(state, 50));
            assert_eq!(browser.state().page_size, 50);
            assert_eq!(browser.state().page, 1);
            assert!(!browser.state().bid_log_page_alias);
            assert!(PageKey::BidLog.write(state, "/bidlog", 3, QueryWriteMode::Push));
            assert_eq!(browser.state().page, 3);
            assert_eq!(browser.state().bid_log_lot.as_deref(), Some("One12"));
        });
    }
}

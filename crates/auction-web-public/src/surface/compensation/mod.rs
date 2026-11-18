//! Public 补偿池页面装配：余额、场次批次与逐件结算明细。

use auction_web_shared::{
    AsyncState, AsyncStateKind, QueryWriteMode, format_credit_amount_full, t,
};
use dioxus::prelude::*;

use super::{
    LotDetailLookupHost, PageKey, PublicBoardQuery, PublicPageClampError,
    commit_public_query_pairs, normalize_public_page_size, page_pathname, public_page_matches,
    public_page_within_offset, read_page_query, use_content_loading, use_lot_detail_lookup,
    use_page_query, use_public_page_bounds,
};
use crate::api::{fetch_compensation_lots, fetch_compensation_pool};
#[cfg(test)]
use crate::surface::PageReadFixture;

mod lots;
mod table;

use table::CompensationPoolTable;

const COMPENSATION_PATH: &str = "/compensation-pool";

/// 批次与展开明细使用各自的页码/数量；批次与明细分页均通过 URL 恢复。
pub(super) fn pool_page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let query_state_signal = use_page_query();
    let page_number = use_memo(move || query_state_signal.read().page.max(1));
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let selected_auction_id =
        use_memo(move || query_state_signal.read().auction_id.clone().unwrap_or_default());
    let movement_page = use_memo(move || query_state_signal.read().movement_page);
    let movement_page_size = use_memo(move || query_state_signal.read().movement_page_size);
    let bounds = use_public_page_bounds(query_state_signal, COMPENSATION_PATH, |query| {
        [(PageKey::Page, query.page_size), (PageKey::Movement, query.movement_page_size)]
    });
    // 逐件明细的查看按钮在页内打开只读的拍品详情抽屉；读取任务归页面宿主，不随翻页卸载。
    let lookup = use_lot_detail_lookup();
    let pool = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let requested = (page_number(), page_size());
            async move {
                if !public_page_within_offset(requested.0, requested.1) {
                    return None;
                }
                #[cfg(test)]
                if let Some(result) = PageReadFixture::response(requested) {
                    return Some((requested, result));
                }
                let result = fetch_compensation_pool(requested.0, requested.1).await;
                Some((requested, result))
            }
        }
    });
    let lots = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = movement_page();
            let page_size = movement_page_size();
            async move {
                if !public_page_within_offset(page, page_size) {
                    return None;
                }
                let result = if auction_id.is_empty() {
                    Ok(None)
                } else {
                    read_lots(auction_id.clone(), page, page_size).await.and_then(|page| {
                        if page.round.auction_id == auction_id {
                            Ok(Some(page))
                        } else {
                            Err("invalid_response".to_owned())
                        }
                    })
                };
                Some((auction_id, page, page_size, result))
            }
        }
    });

    let loaded = pool.read_unchecked();
    let current = loaded
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|(requested, _)| *requested == (page_number(), page_size()))
        .map(|(_, result)| result);
    // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
    let clamp_failed = bounds.failed(PageKey::Page);
    use_content_loading(match current {
        None => !clamp_failed,
        Some(Ok(page)) => {
            !public_page_matches(page.page, page.page_size, page_number(), page_size())
        }
        Some(Err(_)) => false,
    });
    let body = match current {
        _ if clamp_failed => rsx! { PublicPageClampError { bounds } },
        None => rsx! {},
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-compensation-pool-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page))
            if !public_page_matches(page.page, page.page_size, page_number(), page_size()) =>
        {
            rsx! {}
        }
        Some(Ok(page)) => {
            // 只保留同一轮次及同一分页请求的快照，避免旧数据触发新页码纠偏。
            let lot_state = lots.read_unchecked().as_ref().and_then(Option::as_ref).and_then(
                |(auction_id, page, size, result)| {
                    (auction_id == &selected_auction_id()
                        && *page == movement_page()
                        && *size == movement_page_size())
                    .then(|| result.clone())
                },
            );
            rsx! {
                CompensationPoolTable {
                    page: page.clone(),
                    selected_auction_id: selected_auction_id(),
                    lots: lot_state,
                    lot_clamp_failed: bounds.failed(PageKey::Movement),
                    on_page_change: move |page| write_round_page(query_state_signal, page, None, QueryWriteMode::Push),
                    on_page_clamp: move |page| write_round_page(query_state_signal, page, None, QueryWriteMode::Replace),
                    on_page_size_change: move |size| write_round_page(query_state_signal, 1, Some(size), QueryWriteMode::Push),
                    on_select_round: move |auction_id| toggle_round(query_state_signal, auction_id),
                    on_lot_page_change: move |page| PageKey::Movement.write(query_state_signal, COMPENSATION_PATH, page, QueryWriteMode::Push),
                    on_lot_page_clamp: move |page| PageKey::Movement.write(query_state_signal, COMPENSATION_PATH, page, QueryWriteMode::Replace),
                    on_lot_page_size_change: move |size: i64| {
                        page_pathname() == COMPENSATION_PATH && commit_public_query_pairs(
                            query_state_signal, &["movement_page", "movement_page_size"],
                            &[("movement_page_size".to_owned(), normalize_public_page_size(size).to_string())],
                            QueryWriteMode::Push,
                        )
                    },
                    on_retry_lots: move |_| {
                        if bounds.failed(PageKey::Movement) {
                            bounds.retry();
                        } else {
                            refresh_generation.set(refresh_generation().saturating_add(1));
                        }
                    },
                }
            }
        }
    };
    rsx! {
        {body}
        if lookup.failed() {
            p { class: "action-feedback action-feedback--error", role: "alert", {t!("data-error")} }
        }
        // 抽屉宿主在批次读取分支之外：翻页或后台重读替换表格时，已打开的详情不被卸载。
        LotDetailLookupHost { lookup }
    }
}

fn toggle_round(state: Signal<PublicBoardQuery>, auction_id: String) {
    if page_pathname() != COMPENSATION_PATH {
        return;
    }
    let open = read_page_query().auction_id.as_deref() == Some(auction_id.as_str());
    let pairs = if open { Vec::new() } else { vec![("auction".to_owned(), auction_id)] };
    // 只有地址接受场次与明细页码的共同更新后，才允许展开或读取新明细。
    commit_public_query_pairs(state, &["auction", "movement_page"], &pairs, QueryWriteMode::Push);
}

async fn read_lots(
    auction_id: String,
    page: i64,
    page_size: i64,
) -> Result<crate::api::CompensationLotPage, String> {
    #[cfg(test)]
    if let Some(result) = PageReadFixture::response((auction_id.clone(), page, page_size)) {
        return result;
    }
    fetch_compensation_lots(auction_id, page, page_size).await
}

fn write_round_page(
    state: Signal<PublicBoardQuery>,
    page: i64,
    size: Option<i64>,
    mode: QueryWriteMode,
) -> bool {
    if page_pathname() != COMPENSATION_PATH {
        return false;
    }
    let mut keys = vec!["page", "auction", "movement_page"];
    let mut pairs = Vec::new();
    if page > 1 {
        pairs.push(("page".to_owned(), page.to_string()));
    }
    if let Some(size) = size {
        keys.push("page_size");
        pairs.push(("page_size".to_owned(), normalize_public_page_size(size).to_string()));
    }
    commit_public_query_pairs(state, &keys, &pairs, mode)
}

/// 补偿池页面统一展示精确金额，不把整千值缩写为 K/M，避免同一列混用单位影响复算。
fn credits(amount: i64) -> String {
    format!("{}c", format_credit_amount_full(amount))
}

/// 以补偿池视角带符号展示金额：流入为 `+`，流出为 `−`。
fn signed_amount(amount: i64) -> String {
    if amount > 0 {
        format!("+{}", credits(amount))
    } else if amount < 0 {
        format!("−{}", credits(amount.saturating_abs()))
    } else {
        credits(0)
    }
}

/// 实际兑付率：补偿支出占名义折扣需求的比例，精确到 0.01%；没有需求时不适用。
fn coverage_label(compensation_out: i64, discount_demand: i64) -> String {
    if discount_demand <= 0 {
        return "—".to_owned();
    }
    let ppm = i128::from(compensation_out.max(0)) * 1_000_000 / i128::from(discount_demand);
    let ppm = i32::try_from(ppm.min(1_000_000) / 100 * 100).unwrap_or(1_000_000);
    auction_web_shared::format_ppm(ppm, false)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_web_shared::QueryWriteMode;
    use dioxus::dioxus_core::{AttributeValue, ElementId, Mutation};
    use dioxus::prelude::*;

    use super::pool_page;
    use crate::api::{CompensationLotPage, CompensationPoolPage, CompensationRound};
    use crate::surface::{
        PageBrowserFixture, PageReadFixture, dispatch_page_event, page_event_target,
        page_test_contexts, rendered_event_target, run_page_events, settle_page,
    };

    const ROUND: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    fn round() -> CompensationRound {
        CompensationRound {
            auction_id: ROUND.to_owned(),
            world_id: "main".to_owned(),
            auction_sequence: 7,
            policy_mode: "hold_curve_v1".to_owned(),
            opening_balance: 0,
            tax_income: 0,
            discount_demand: 0,
            compensation_out: 0,
            closing_balance: 0,
            movement_count: 0,
            reversal_count: 0,
            reversal_amount: 0,
            settled_at: "2026-11-16T10:00:00Z".to_owned(),
        }
    }

    fn compensation() -> Element {
        page_test_contexts();
        pool_page()
    }

    fn changed(dom: &VirtualDom, select: ElementId, size: i64) {
        dispatch_page_event(dom, "change", select, &size.to_string());
    }

    fn assert_rewrote_select(edits: &[Mutation], select: ElementId, applied: i64) {
        assert!(edits.iter().any(|edit| matches!(
            edit, Mutation::SetAttribute { name: "value", value: AttributeValue::Text(value), id, .. }
                if *id == select && value == &applied.to_string()
        )), "拒写后应以已应用数量重写同一原生下拉框");
    }

    fn click_page(dom: &mut VirtualDom, label: &str) {
        let button = rendered_event_target(dom, "button", "aria-label", label, "onclick");
        dispatch_page_event(dom, "click", button, "");
        settle_page(dom);
    }

    fn assert_page_write(
        browser: &PageBrowserFixture,
        before: usize,
        mode: QueryWriteMode,
        keys: &[&str],
        value: &str,
    ) {
        let writes = browser.writes();
        assert_eq!(writes.len(), before + 1);
        let write = &writes[before];
        assert!(write.accepted);
        assert_eq!(write.mode, mode);
        assert_eq!(write.keys, keys);
        assert_eq!(write.pairs, [(keys[0].to_owned(), value.to_owned())]);
    }

    #[test]
    fn compensation_events_drive_the_round_and_both_page_size_writes() {
        run_page_events(
            module_path!(),
            "compensation_events_drive_the_round_and_both_page_size_writes",
            exercise_controls,
        );
    }

    fn exercise_controls() {
        let browser = PageBrowserFixture::new(
            "/compensation-pool",
            "page=2&page_size=20&movement_page=3&movement_page_size=50&keep=yes",
        );
        let pool_requests = Rc::new(RefCell::new(Vec::new()));
        let detail_requests = Rc::new(RefCell::new(Vec::new()));
        let detail_total = Rc::new(Cell::new(300));
        let pool_reads = pool_requests.clone();
        let pool = PageReadFixture::new(move |(page, page_size): (i64, i64)| {
            pool_reads.borrow_mut().push((page, page_size));
            Ok::<_, String>(CompensationPoolPage {
                balance: 0,
                updated_at: round().settled_at,
                rounds: vec![round()],
                page: Some(page),
                page_size,
                total: Some(100),
                cursor: None,
                next_cursor: None,
                has_more: None,
            })
        });
        let detail_reads = detail_requests.clone();
        let total = detail_total.clone();
        let details =
            PageReadFixture::new(move |(auction, page, page_size): (String, i64, i64)| {
                assert_eq!(auction, ROUND);
                detail_reads.borrow_mut().push((page, page_size));
                Ok::<_, String>(CompensationLotPage {
                    round: round(),
                    lots: Vec::new(),
                    total: total.get(),
                    page,
                    page_size,
                })
            });
        let mut dom = VirtualDom::new(compensation)
            .with_root_context(browser.clone())
            .with_root_context(pool)
            .with_root_context(details);
        let mut edits = dom.rebuild_to_vec().edits;
        edits.extend(settle_page(&mut dom));
        let expand = page_event_target(
            &edits,
            "click",
            "aria-controls",
            &format!("compensation-details-{ROUND}"),
        );
        let main_select = page_event_target(&edits, "change", "value", "20");

        browser.allow_writes(false);
        dispatch_page_event(&dom, "click", expand, "");
        settle_page(&mut dom);
        assert_eq!(browser.state().auction_id, None);
        assert_eq!(browser.state().movement_page, 3);
        assert!(detail_requests.borrow().is_empty());
        assert_eq!(browser.writes()[0].keys, ["auction", "movement_page"]);
        assert!(!browser.writes()[0].accepted);

        browser.allow_writes(true);
        dispatch_page_event(&dom, "click", expand, "");
        let edits = settle_page(&mut dom);
        let detail_select = page_event_target(&edits, "change", "value", "50");
        assert_ne!(main_select, detail_select);
        assert_eq!(browser.state().auction_id.as_deref(), Some(ROUND));
        assert_eq!(browser.state().movement_page, 1);
        assert_eq!(&*detail_requests.borrow(), &[(1, 50)]);

        browser.allow_writes(false);
        let before = browser.pairs();
        for (select, applied, next, key) in
            [(main_select, 20, 50, "page_size"), (detail_select, 50, 100, "movement_page_size")]
        {
            let attempts = browser.writes().len();
            changed(&dom, select, next);
            let edits = settle_page(&mut dom);
            let writes = browser.writes();
            assert_eq!(writes.len(), attempts + 1, "下拉必须走实际页面回调");
            assert_eq!(writes[attempts].mode, QueryWriteMode::Push);
            assert!(!writes[attempts].accepted);
            assert_eq!(writes[attempts].pairs, [(key.to_owned(), next.to_string())]);
            assert_eq!(browser.pairs(), before);
            assert_eq!(browser.sizes(), [20, 20]);
            assert_eq!(browser.state().page_size, 20);
            assert_eq!(browser.state().movement_page_size, 50);
            assert_rewrote_select(&edits, select, applied);
        }

        browser.allow_writes(true);
        changed(&dom, detail_select, 100);
        let edits = settle_page(&mut dom);
        assert_eq!(browser.state().movement_page_size, 100);
        assert_eq!(browser.sizes(), [20, 100]);
        assert_eq!(&*detail_requests.borrow(), &[(1, 50), (1, 100)]);
        assert!(edits.iter().any(|edit| matches!(
            edit, Mutation::SetAttribute { name: "value", value: AttributeValue::Text(value), .. }
                if value == "100"
        )));

        let mut expected = browser.state();
        let before = browser.writes().len();
        click_page(&mut dom, "Page 3 / 3");
        assert_page_write(&browser, before, QueryWriteMode::Push, &["movement_page"], "3");
        assert_eq!(browser.value("movement_page").as_deref(), Some("3"));
        assert_eq!(browser.value("page").as_deref(), Some("2"));
        assert_eq!(browser.value("auction").as_deref(), Some(ROUND));
        expected.movement_page = 3;
        assert_eq!(browser.state(), expected);
        assert_eq!(&*detail_requests.borrow(), &[(1, 50), (1, 100), (3, 100)]);
        assert_eq!(&*pool_requests.borrow(), &[(2, 20)]);

        detail_total.set(200);
        let before = browser.writes().len();
        let other_pairs = |mut pairs: Vec<(String, String)>| {
            pairs.retain(|(key, _)| key != "movement_page");
            pairs
        };
        let before_pairs = other_pairs(browser.pairs());
        dom.in_scope(ScopeId::APP, || {
            let mut refresh = consume_context::<Signal<u64>>();
            refresh.set(refresh().saturating_add(1));
        });
        settle_page(&mut dom);
        assert_page_write(&browser, before, QueryWriteMode::Replace, &["movement_page"], "2");
        assert_eq!(browser.value("movement_page").as_deref(), Some("2"));
        assert_eq!(other_pairs(browser.pairs()), before_pairs);
        expected.movement_page = 2;
        assert_eq!(browser.state(), expected);
        assert_eq!(&*detail_requests.borrow(), &[(1, 50), (1, 100), (3, 100), (3, 100), (2, 100)]);

        let before = browser.writes().len();
        let detail_reads = detail_requests.borrow().len();
        click_page(&mut dom, "Page 3 / 5");
        assert_page_write(
            &browser,
            before,
            QueryWriteMode::Push,
            &["page", "auction", "movement_page"],
            "3",
        );
        assert_eq!(browser.value("page").as_deref(), Some("3"));
        assert_eq!(browser.value("auction"), None);
        assert_eq!(browser.value("movement_page"), None);
        expected.page = 3;
        expected.auction_id = None;
        expected.movement_page = 1;
        assert_eq!(browser.state(), expected);
        assert_eq!(&*pool_requests.borrow(), &[(2, 20), (2, 20), (3, 20)]);
        assert_eq!(detail_requests.borrow().len(), detail_reads);

        let main_select = rendered_event_target(&dom, "select", "value", "20", "onchange");
        changed(&dom, main_select, 50);
        settle_page(&mut dom);
        assert_eq!(browser.state().page_size, 50);
        assert_eq!(browser.state().page, 1);
        assert_eq!(browser.state().auction_id, None);
        assert_eq!(browser.sizes(), [50, 100]);
        assert_eq!(browser.value("keep").as_deref(), Some("yes"));
    }
}

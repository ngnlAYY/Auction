//! Realtime failure 表格与分页组件。

use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind, Time};
use dioxus::prelude::*;

use super::filter_field::fetch_filtered;
use crate::api::{RealtimeFailure, RealtimeFailurePage, fetch_realtime_failure_page};
use crate::surface::pagination::{AdminPagination, use_admin_page_with_state};
use crate::surface::pending::PendingCommands;

type FailurePageRequest = (i64, String, String, String, String);

/// 失败列表和未解决计数共享失败分区的刷新世代。
pub(super) struct FailureReads {
    pub(super) page: Memo<Option<Result<RealtimeFailurePage, String>>>,
    pub(super) status: ReadSignal<UseResourceState>,
    pub(super) unresolved: Resource<Result<RealtimeFailurePage, String>>,
}

/// 读取当前筛选的失败列表及独立的未解决计数。
pub(super) fn use_failure_reads(
    pagination: AdminPagination,
    failure_query: Memo<Option<FailurePageRequest>>,
    failure_allowed: Memo<bool>,
    failure_refresh: Signal<u64>,
    replay_refresh: Signal<u64>,
) -> FailureReads {
    let (page, status) = use_admin_page_with_state(
        pagination,
        move || {
            let _ = replay_refresh();
            let _ = failure_refresh();
            failure_query()
        },
        |request, size| {
            fetch_filtered(request, move |(page, stream, consumer, code, state)| {
                fetch_realtime_failure_page(page, stream, consumer, code, state, size)
            })
        },
    );
    let unresolved = use_resource(move || {
        let _ = replay_refresh();
        let _ = failure_refresh();
        fetch_filtered(failure_allowed().then_some(()), |()| {
            fetch_realtime_failure_page(
                1,
                String::new(),
                String::new(),
                String::new(),
                "dead_letter".to_owned(),
                10,
            )
        })
    });
    FailureReads { page, status, unresolved }
}

/// 列表和弹窗共用只读重试，仅更新失败分区，保留已选目标与草稿。
pub(super) fn retry_failure_read(status: UseResourceState, mut refresh: Signal<u64>) {
    if status == UseResourceState::Ready {
        refresh.set(refresh().saturating_add(1));
    }
}

/// 弹窗的错误态重试入口沿用列表的失败分区刷新世代。
#[component]
pub(super) fn FailureReadRetry(
    status: ReadSignal<UseResourceState>,
    refresh: Signal<u64>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "admin-btn-secondary",
            disabled: status.cloned() != UseResourceState::Ready,
            onclick: move |_| retry_failure_read(status.cloned(), refresh),
            {t!("admin-retry")}
        }
    }
}

/// 展示可供操作员选择 replay 的 failure 安全投影。
#[component]
pub(super) fn RealtimeFailureTable(
    page: RealtimeFailurePage,
    on_select: EventHandler<RealtimeFailure>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let is_empty = page.failures.is_empty();
    // 操作负载与错误详情在 rsx 外配好，循环体根节点才能直接是带 key 的行。
    let rows = page.failures.into_iter().map(|failure| {
        let failure_for_action = failure.clone();
        let error_detail = format!("{}: {}", failure.code, failure.safe_detail);
        (failure, failure_for_action, error_detail)
    });
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-realtime-table",
                aria_label: t!("realtime-failure-title"),
                auction_web_shared::TableColumns { layout: "admin-realtime-failures", widths: vec![17, 20, 17, 9, 12, 12, 7, 6] }
                thead {
                    tr {
                        th { scope: "col", {t!("realtime-failure-time")} }
                        th { scope: "col", {t!("realtime-failure-error")} }
                        th { scope: "col", {t!("realtime-stream")} }
                        th { scope: "col", {t!("realtime-consumer")} }
                        th { scope: "col", {t!("realtime-failure-event-id")} }
                        th { scope: "col", {t!("realtime-failure-id")} }
                        th { scope: "col", {t!("realtime-failure-state")} }
                        th { scope: "col", {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for (failure, failure_for_action, error_detail) in rows {
                        // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                        tr { key: "{failure.id}", class: "admin-realtime__row",
                            td {
                                Time { value: failure.failed_at.clone() }
                            }
                            td { class: "admin-realtime__error",
                                TruncatedText {
                                    display: failure.code.clone(),
                                    size: "wide",
                                    value: error_detail,
                                }
                            }
                            td { TruncatedText { value: failure.stream_key.clone() } }
                            td { TruncatedText { value: failure.consumer_key.clone() } }
                            td {
                                TruncatedText {
                                    display: compact_realtime_id(&failure.event_id),
                                    size: "compact",
                                    value: failure.event_id.clone()
                                }
                            }
                            td {
                                TruncatedText {
                                    display: compact_realtime_id(&failure.id),
                                    size: "compact",
                                    value: failure.id.clone()
                                }
                            }
                            td {
                                span {
                                    class: if failure.resolved {
                                        "admin-realtime__status admin-realtime__status--resolved"
                                    } else {
                                        "admin-realtime__status admin-realtime__status--unresolved"
                                    },
                                    if failure.resolved {
                                        {t!("realtime-state-completed")}
                                    } else {
                                        {t!("realtime-state-dead-letter")}
                                    }
                                }
                            }
                            td {
                                div { class: "admin-realtime__actions",
                                    if !failure.resolved {
                                        button {
                                            r#type: "button",
                                            disabled: pending.any_busy() || pending.contains(&format!("realtime-failure:{}", failure.id)),
                                            onclick: move |_| on_select.call(failure_for_action.clone()),
                                            {t!("realtime-replay")}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("realtime-failure-empty"),
                detail: "",
            }
        }
    }
}

fn compact_realtime_id(value: &str) -> String {
    if value.len() <= 12 {
        value.to_owned()
    } else {
        let start = value.char_indices().rev().nth(7).map_or(0, |(index, _)| index);
        value[start..].to_owned()
    }
}

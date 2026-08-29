//! Public lot 详情页面装配。

use super::super::*;

mod detail;
mod equipment;
mod history;

pub(crate) use detail::{LiveLotDetailView, LotDetailBack, LotDetailHeader, PreviewLotDetailView};
pub(crate) use equipment::{EquipmentCard, EquipmentCardData};
pub(crate) use history::BidHistoryTable;

#[component]
pub(crate) fn LotDetail(lot_code: String) -> Element {
    if !is_public_lot_id(&lot_code) {
        return rsx! { LotMissing {} };
    }
    rsx! { ValidLotDetail { lot_id: lot_code } }
}

/// 旧版浏览器只接受小写、带连字符、RFC 4122 variant 且版本为 1 至 5 的 UUID。
fn is_public_lot_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(*byte, b'a'..=b'f')
            }
        })
        && matches!(bytes[14], b'1'..=b'5')
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

#[component]
fn ValidLotDetail(lot_id: String) -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let lot_id_for_request = lot_id.clone();
    let detail = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = lot_id_for_request.clone();
        async move { fetch_lot_detail(lot_id).await }
    });
    let detail_for_poll = detail;
    let should_poll = use_memo(move || match &*detail_for_poll.read_unchecked() {
        None | Some(Err(_)) => true,
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => {
                matches!(lot.summary.status.as_str(), "pending" | "live")
            }
            Some(LotDetailItem::Live(lot)) => matches!(lot.status.as_str(), "pending" | "live"),
            None => true,
        },
    });
    let _compatibility_poll = use_future(move || async move {
        loop {
            TimeoutFuture::new(60_000).await;
            // 旧页面只在实时连接正常时保留一分钟兜底轮询；manual/degraded 模式不会
            // 自动重放永久 404 或服务错误，用户仍可通过隐藏的键盘重试入口显式刷新。
            if realtime_state() == LiveConnectionState::Live && should_poll() {
                refresh_generation.set(refresh_generation().saturating_add(1));
            }
        }
    });

    match &*detail.read_unchecked() {
        None => rsx! {
            p { class: "public-page-status", {t!("common-loading")} }
        },
        Some(Err(error)) => rsx! {
            LotLoadFailure {
                detail: lot_detail_error_message(error),
                refresh_generation,
            }
        },
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => rsx! {
                PreviewLotDetailView { lot: lot.clone() }
            },
            Some(LotDetailItem::Live(lot)) => rsx! {
                LiveLotDetailView {
                    auction_id: page.auction_id.clone(),
                    auction_status: page.auction_status.clone(),
                    bidding_open: page.bidding_open,
                    lot: lot.clone(),
                }
            },
            None => rsx! {
                LotLoadFailure {
                    detail: t!("error-invalid-response"),
                    refresh_generation,
                }
            },
        },
    }
}

#[component]
fn LotMissing() -> Element {
    rsx! {
        div { class: "ui-empty-state", role: "status",
            p { class: "ui-empty-state__title", {t!("lot-not-found")} }
        }
    }
}

#[component]
fn LotLoadFailure(detail: String, mut refresh_generation: Signal<u64>) -> Element {
    rsx! {
        div { class: "ui-empty-state", role: "alert",
            p { class: "ui-empty-state__title", {t!("lot-not-found")} }
            span { class: "ui-empty-state__detail", "{detail}" }
            button {
                class: "ui-empty-state__retry",
                r#type: "button",
                onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                {t!("data-retry")}
            }
        }
    }
}

fn lot_detail_error_message(error: &LotDetailFetchError) -> String {
    match (error.status(), error.code()) {
        (_, "invalid_response") => t!("error-invalid-response"),
        (0, "network_error") => t!("error-network"),
        (0, "request_timeout") => t!("error-request-timeout"),
        (0, "request_cancelled") => t!("error-request-cancelled"),
        (404, "not_found" | "NOT_FOUND") => t!("error-not-found"),
        (500, "internal" | "INTERNAL") => t!("error-internal"),
        _ => t!("common-load-error"),
    }
}

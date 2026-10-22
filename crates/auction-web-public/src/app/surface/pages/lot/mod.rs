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
    // Router 会复用详情路由组件；目标 UUID 改变时销毁旧读取与命令宿主。
    rsx! { ValidLotDetail { key: "{lot_code}", lot_id: lot_code } }
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
    let lot_id_for_request = lot_id.clone();
    let detail = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = lot_id_for_request.clone();
        async move { fetch_lot_detail(lot_id).await }
    });
    use_bid_refresh(detail, true);

    match &*detail.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("common-loading"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("lot-not-found"),
                detail: lot_detail_error_message(error),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => rsx! {
                LotWatchScope {
                    auction_id: page.auction_id.clone(),
                    PreviewLotDetailView { lot: lot.clone() }
                }
            },
            Some(LotDetailItem::Live(lot)) => rsx! {
                LotWatchScope {
                    auction_id: page.auction_id.clone(),
                    LiveLotDetailView {
                        auction_id: page.auction_id.clone(),
                        auction_status: page.auction_status.clone(),
                        bidding_open: page.bidding_open,
                        lot: lot.clone(),
                    }
                }
            },
            None => rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("lot-not-found"),
                    detail: t!("error-invalid-response"),
                    retry_label: t!("data-retry"),
                    on_retry: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
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

/// lot 详情读取失败的本地化文案：先处理带状态约束的历史变体码，未命中走共享基础层
/// （[`base_api_error_message`]），最后落到通用兜底。
fn lot_detail_error_message(error: &LotDetailFetchError) -> String {
    match (error.status(), error.code()) {
        (0, "request_cancelled") => return t!("error-request-cancelled"),
        (404, "NOT_FOUND") => return t!("error-not-found"),
        (500, "internal" | "INTERNAL") => return t!("error-internal"),
        _ => {}
    }
    base_api_error_message(error.code(), |kind| match kind {
        BaseErrorKind::Network => t!("error-network"),
        BaseErrorKind::Timeout => t!("error-request-timeout"),
        BaseErrorKind::InvalidResponse => t!("error-invalid-response"),
        BaseErrorKind::NotFound => t!("error-not-found"),
        _ => t!("common-load-error"),
    })
    .unwrap_or_else(|| t!("common-load-error"))
}

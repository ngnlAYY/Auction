//! Public lot 详情页面装配。

use super::super::*;

mod detail;
mod equipment;
mod history;

pub(crate) use detail::{LiveLotDetailView, PreviewLotDetailView};
pub(crate) use equipment::{EquipmentCard, EquipmentCardData};
pub(crate) use history::BidHistoryTable;

#[component]
pub(crate) fn LotDetail(lot_code: String) -> Element {
    if !auction_web_shared::is_legacy_uuid(&lot_code) {
        return rsx! { LotMissing {} };
    }
    // Router 会复用详情路由组件；目标 UUID 改变时销毁旧读取与命令宿主。
    rsx! {
        for lot_code in [lot_code] {
            CatalogCommandScope {
                key: "{lot_code}",
                ValidLotDetail { lot_id: lot_code }
            }
        }
    }
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
    let auction_identity = use_memo(move || {
        detail.read().as_ref().and_then(|result| {
            result.as_ref().ok().map(|page| {
                (page.auction_id.clone(), page.auction_status.clone(), page.preview_revision)
            })
        })
    });
    let auction = use_resource(move || {
        let _ = refresh_generation();
        let identity = auction_identity();
        async move {
            match identity {
                Some(identity) => {
                    let result = fetch_historical_auction(identity.0.clone()).await;
                    Some((identity, result))
                }
                None => None,
            }
        }
    });
    let mut realtime_target = use_context::<PublicPageRealtimeTarget>().0;
    let query = use_public_query_state().state;
    use_effect({
        let lot_id = lot_id.clone();
        move || {
            let identity = auction_identity();
            let detail_failed = detail.read().as_ref().is_some_and(Result::is_err);
            let loaded = auction.read();
            let resolved = loaded
                .as_ref()
                .and_then(Option::as_ref)
                .filter(|(requested, _)| Some(requested) == identity.as_ref());
            let target = match resolved {
                Some((expected, Ok(auction)))
                    if auction.id == expected.0 && auction.status == expected.1 =>
                {
                    // URL 世界只保留导航意图；实际房间与返回目录都由拍卖 UUID 的权威读取绑定。
                    let current = query();
                    let world = normalize_world_query(&auction.world);
                    if current.world != world
                        || current.auction_id.as_deref() != Some(auction.id.as_str())
                    {
                        update_public_query_state(
                            query,
                            |query| {
                                query.world = world;
                                query.auction_id = Some(auction.id.clone());
                            },
                            QueryWriteMode::Replace,
                        );
                    }
                    PublicRealtimeTarget::from_auction(auction)
                }
                Some(_) => PublicRealtimeTarget::IdentityError { target_id: lot_id.clone() },
                None if detail_failed => {
                    PublicRealtimeTarget::IdentityError { target_id: lot_id.clone() }
                }
                None => PublicRealtimeTarget::Checking,
            };
            let next = Some((PublicRealtimePageKey::Lot(lot_id.clone()), target));
            if *realtime_target.peek() != next {
                realtime_target.set(next);
            }
        }
    });
    use_drop(move || {
        if let Ok(mut current) = realtime_target.try_write()
            && matches!(current.as_ref(), Some((PublicRealtimePageKey::Lot(loaded), _)) if loaded == &lot_id)
        {
            *current = None;
        }
    });

    let detail_snapshot = detail.read_unchecked();
    use_content_loading(detail_snapshot.is_none());
    match &*detail_snapshot {
        None => rsx! {},
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
                        // 重读期间沿用同一 lot 的上一份权威详情，避免竞价通知让操作按钮闪烁。
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
        div { class: "ui-async-state", role: "status",
            p { class: "ui-async-state__title", {t!("lot-not-found")} }
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

//! 详情抽屉里的本人代理授权摘要：上限、已用次数、模式与状态。
//! 只展示服务端按会话返回的本人授权；上限不公开，抽屉与页面级信号都由身份宿主持有，
//! 身份切换时一并清空。读取由行内操作栏在抽屉打开时发起，这里不发请求、不推导状态。

use auction_web_shared::{format_credit_amount as format_amount, legacy_t as t};
use dioxus::prelude::*;

use crate::api::OwnedProxy;
use crate::surface::proxy::{proxy_mode_label, proxy_quota_label, proxy_status_label};

use super::CatalogDetailBid;

/// 从出价入口投影里取出本人对该拍品的代理授权。
///
/// 只有已登录的命令入口带授权快照；读取中、读取失败或没有设置授权时返回 `None`，
/// 抽屉不为它们占位，错误与重试留给代理弹窗。
pub(super) fn own_proxy(bid: &CatalogDetailBid) -> Option<&OwnedProxy> {
    match bid {
        CatalogDetailBid::Command { input, .. } => match input.proxy.as_ref() {
            Some(Ok(Some(proxy))) => Some(proxy),
            _ => None,
        },
        CatalogDetailBid::Login | CatalogDetailBid::Unavailable => None,
    }
}

/// “我的代理出价”分区；样式沿用抽屉的事实网格与分区标题。
#[component]
pub(super) fn DetailOwnProxy(proxy: OwnedProxy) -> Element {
    let title = t!("proxy-summary-title");
    let status_class = match proxy.status.as_str() {
        "active" => "active",
        "final_fast" => "final-fast",
        "exhausted" => "exhausted",
        "frozen" => "frozen",
        "closed" => "closed",
        _ => "unknown",
    };
    rsx! {
        section { class: "public-catalog-detail__section", aria_label: title.clone(),
            div { class: "public-catalog-detail__section-head",
                h3 { class: "public-catalog-detail__section-title", "{title}" }
            }
            dl { class: "public-catalog-detail__facts",
                div { dt { {t!("account-maximum")} } dd { "{format_amount(proxy.maximum_amount)}" } }
                div { dt { {t!("account-proxy-uses")} } dd { {proxy_quota_label(&proxy)} } }
                div { dt { {t!("account-mode")} } dd { {proxy_mode_label(&proxy.mode)} } }
                div { dt { {t!("account-status")} }
                    dd {
                        span {
                            class: "public-auction-proxy-status public-auction-proxy-status--{status_class}",
                            {proxy_status_label(&proxy.status)}
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::OwnedProxy;
    use crate::surface::catalog::command_scope::{CatalogAction, CommandDialogInput};

    use super::{CatalogDetailBid, own_proxy};

    fn proxy() -> OwnedProxy {
        serde_json::from_value(serde_json::json!({
            "authorization_id": "018f4c8e-7b2a-7c3d-8e4f-000000000001",
            "auction_id": "018f4c8e-7b2a-7c3d-8e4f-000000000002",
            "lot_id": "018f4c8e-7b2a-7c3d-8e4f-000000000003",
            "lot_number": 7,
            "authorization_sequence": 1,
            "usage_number": 2,
            "mode": "fast",
            "maximum_amount": 250_000,
            "quota": { "used": 2, "limit": 3 },
            "transitions": [],
            "status": "active",
            "state_version": 4,
            "created_at": "2026-11-13T00:00:00Z",
            "updated_at": "2026-11-13T00:00:00Z",
            "equipment": { "name": "Legendary Shield", "category": "shield" },
            "capabilities": { "can_lower": true, "waiting": false, "at_maximum": false }
        }))
        .expect("fixture matches the owned proxy contract")
    }

    fn command(proxy: Option<Result<Option<OwnedProxy>, String>>) -> CatalogDetailBid {
        CatalogDetailBid::Command {
            input: Box::new(CommandDialogInput {
                lot_id: "018f4c8e-7b2a-7c3d-8e4f-000000000003".to_owned(),
                lot_code: "S07".to_owned(),
                lot_name: "Legendary Shield".to_owned(),
                action: CatalogAction::Bid,
                can_bid: true,
                current_price: 100_000,
                next_bid: 105_000,
                lot_status: "live".to_owned(),
                allow_slow: true,
                ordinary_blocked: false,
                leading: false,
                blocked_reason: String::new(),
                proxy,
            }),
            disabled: false,
        }
    }

    #[test]
    fn drawer_shows_the_own_proxy_only_after_it_was_read() {
        // 回归：抽屉取代拍品页之后，本人代理的上限与已用次数仍要在详情里可见。
        let loaded = command(Some(Ok(Some(proxy()))));
        let shown = own_proxy(&loaded).expect("loaded authorization is shown");
        assert_eq!((shown.maximum_amount, shown.quota.used, shown.quota.limit), (250_000, 2, 3));

        // 读取中、读取失败与没有设置授权都不占位。
        assert!(own_proxy(&command(None)).is_none());
        assert!(own_proxy(&command(Some(Err("unauthenticated".to_owned())))).is_none());
        assert!(own_proxy(&command(Some(Ok(None)))).is_none());
        // 匿名访客与没有操作栏的行没有授权快照。
        assert!(own_proxy(&CatalogDetailBid::Login).is_none());
        assert!(own_proxy(&CatalogDetailBid::Unavailable).is_none());
    }
}

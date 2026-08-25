//! Public lot 竞价历史展示。

use super::*;

#[component]
pub(crate) fn BidHistoryTable(page: crate::api::BidLogPage) -> Element {
    rsx! {
        div { class: "public-auction-lot__table",
            table {
                thead {
                    tr {
                        th { {t!("bidlog-bidder")} }
                        th { class: "public-auction-lot__number", {t!("bidlog-amount")} }
                        th { {t!("bidlog-mechanism")} }
                        th { {t!("bidlog-time")} }
                    }
                }
                tbody {
                    for bid in page.bids {
                        tr { class: if bid.valid { "public-auction-lot__leading" } else { "" },
                            td { class: "public-auction-lot__participant-cell", "data-label": t!("bidlog-bidder"),
                                CatalogParticipant {
                                    name: bid.bidder_name.clone(),
                                    marker: (bid.mechanism == "proxy").then(|| t!("catalog-proxy-marker")),
                                    trailing: None,
                                }
                            }
                            td { class: "public-auction-lot__amount public-auction-lot__number", "data-label": t!("bidlog-amount"), "{format_amount(bid.amount)}" }
                            td { "data-label": t!("bidlog-mechanism"), "{bid_mechanism_label(&bid.mechanism)}" }
                            td { "data-label": t!("bidlog-time"), time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" } }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn bid_mechanism_label(mechanism: &str) -> String {
    match mechanism {
        "proxy" => t!("bidlog-mechanism-proxy"),
        _ => t!("bidlog-mechanism-ordinary"),
    }
}

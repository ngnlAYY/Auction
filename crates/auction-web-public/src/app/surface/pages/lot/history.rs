//! Public lot 竞价历史展示。

use super::*;

#[component]
pub(crate) fn BidHistoryTable(page: crate::api::BidLogPage) -> Element {
    rsx! {
        div { class: "public-auction-lot__table",
            table { class: "ui-table--responsive",
                caption { class: "public-visually-hidden", {t!("page-bidlog-title")} }
                auction_web_shared::TableColumns { layout: "public-lot-bid-history", widths: vec![38, 22, 40] }
                thead {
                    tr {
                        th { {t!("bidlog-bidder")} }
                        th { class: "public-auction-lot__number", {t!("bidlog-amount")} }
                        th { {t!("bidlog-time")} }
                    }
                }
                tbody {
                    for bid in page.bids {
                        tr { class: if bid.valid { "public-auction-lot__leading" } else { "" },
                            td { class: "public-auction-lot__participant-cell", "data-label": t!("bidlog-bidder"),
                                BidHistoryParticipant {
                                    name: bid.bidder_name.clone(),
                                    source: bid.source.clone(),
                                    source_post_number: bid.source_post_number,
                                    source_post_url: bid.source_post_url.clone(),
                                }
                                span { class: "public-visually-hidden",
                                    if bid.valid { {t!("bidlog-valid")} } else { {t!("bidlog-invalid")} }
                                }
                            }
                            td { class: "public-auction-lot__amount public-auction-lot__number", "data-label": t!("bidlog-amount"), "{format_amount(bid.amount)}" }
                            td { "data-label": t!("bidlog-time"), Time { value: bid.created_at.clone() } }
                        }
                    }
                }
            }
        }
    }
}

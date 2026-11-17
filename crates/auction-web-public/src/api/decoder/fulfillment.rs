//! 履约进度路由的 data 校验：字段闭集、状态闭集，以及按状态成对或条件出现的可选字段。
//!
//! 条件与服务端判定一一对应：流拍状态没有成交事实与退回次数；成交状态必有买家与成交价；付款
//! 期限只随三种付款等待状态出现，且是与目标无关的严格 UTC 秒格式；退回次数与上限成对出现且
//! 计数不超过上限，两种首次付款状态（待寄出 COD、等待确认收款）不带退回次数；非空页按请求页码
//! 算出的 offset 加本页行数不超过总数。浏览器只校验这些形状，不重建状态判定。

use auction_contracts::PublicFulfillmentPageResponse;
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_keys_only;
use gloo_net::http::Response;
use serde_json::{Map, Value};

use super::{account_nonempty, decode_success, int, object, positive, string};

/// 履约进度状态闭集；顺序与 docs/contracts/routes.md 的状态表一致。
pub(crate) const FULFILLMENT_STATUSES: [&str; 31] = [
    "unsold_return_pending",
    "unsold_return_sent",
    "unsold_returned",
    "unsold_return_on_hold",
    "payment_mail_pending",
    "awaiting_payment",
    "payment_overdue",
    "payment_on_hold",
    "returned_by_buyer",
    "returned_on_hold",
    "cod_resend_pending",
    "cod_resent_awaiting_payment",
    "cod_return_limit_reached",
    "handoff_pending",
    "handoff_sent",
    "handoff_on_hold",
    "handoff_returned_by_buyer",
    "payout_pending",
    "payout_sent",
    "payout_on_hold",
    "completed",
    "equipment_lost",
    "voided_return_pending",
    "voided_return_sent",
    "voided_returned",
    "voided_on_hold",
    "voided_refund_pending",
    "voided_relisted",
    "voided_relist_pending",
    "voided_relist_unavailable",
    "unknown",
];

/// 解码履约进度编号页，并把响应绑定到请求的场次、世界与分页。
pub(crate) async fn decode_fulfillment_page(
    response: Response,
    auction_id: &str,
    world: &str,
    page: i64,
    page_size: i64,
) -> Result<PublicFulfillmentPageResponse, String> {
    decode_success(response, |value| {
        valid_fulfillment_page(value, auction_id, world, page, page_size)
    })
    .await
}

fn valid_fulfillment_page(
    value: &Value,
    auction_id: &str,
    world: &str,
    page: i64,
    page_size: i64,
) -> bool {
    let Some(value) = object(value) else { return false };
    let Some(total) = int(value.get("total")).filter(|total| *total >= 0) else { return false };
    json_keys_only(value, &["auction", "lots", "page", "page_size", "total"], &[])
        && int(value.get("page")) == Some(page)
        && int(value.get("page_size")) == Some(page_size)
        && value.get("auction").is_some_and(|auction| valid_auction(auction, auction_id, world))
        && value.get("lots").and_then(Value::as_array).is_some_and(|lots| {
            i64::try_from(lots.len()).is_ok_and(|count| {
                count <= page_size
                    && (count == 0 || page_within_total(page, page_size, count, total))
            }) && lots.iter().all(valid_lot)
                && lots.iter().enumerate().all(|(index, lot)| {
                    lots[..index].iter().all(|other| other.get("lot_id") != lot.get("lot_id"))
                })
        })
}

/// 非空页的末行序号不超过总数：按请求页码与页大小算出的 offset 加本页行数不得超过 `total`。
/// 越过末页的空页仍然有效（筛选后总数变少或旧书签），由分页器纠偏页码。
fn page_within_total(page: i64, page_size: i64, count: i64, total: i64) -> bool {
    page.checked_sub(1)
        .filter(|previous_pages| *previous_pages >= 0)
        .and_then(|previous_pages| previous_pages.checked_mul(page_size))
        .and_then(|offset| offset.checked_add(count))
        .is_some_and(|end| end <= total)
}

fn valid_auction(value: &Value, auction_id: &str, world: &str) -> bool {
    object(value).is_some_and(|auction| {
        json_keys_only(auction, &["id", "world", "number"], &[])
            && string(auction.get("id")) == Some(auction_id)
            && valid_uuid(auction_id)
            && string(auction.get("world")) == Some(world)
            && positive(auction.get("number"))
    })
}

fn valid_lot(value: &Value) -> bool {
    let Some(lot) = object(value) else { return false };
    let Some(status) =
        string(lot.get("status")).filter(|status| FULFILLMENT_STATUSES.contains(status))
    else {
        return false;
    };
    json_keys_only(
        lot,
        &["lot_id", "lot_number", "equipment_name", "equipment_category", "seller_name", "status"],
        &[
            "buyer_name",
            "final_price",
            "payment_deadline_at",
            "cod_return_count",
            "cod_return_limit",
        ],
    ) && valid_uuid(string(lot.get("lot_id")).unwrap_or_default())
        && positive(lot.get("lot_number"))
        && account_nonempty(lot.get("equipment_name"))
        && account_nonempty(lot.get("equipment_category"))
        && account_nonempty(lot.get("seller_name"))
        && valid_sale(lot, status)
        && valid_payment_deadline(lot, status)
        && valid_cod_returns(lot, status)
}

/// 流拍状态不公开成交事实；除“未知”外的成交状态必有买家与成交价，两者成对。
fn valid_sale(lot: &Map<String, Value>, status: &str) -> bool {
    match (lot.get("buyer_name"), lot.get("final_price")) {
        (None, None) => !sold_status(status),
        (Some(buyer), Some(price)) => {
            !unsold_status(status) && account_nonempty(Some(buyer)) && positive(Some(price))
        }
        _ => false,
    }
}

/// 付款期限只随三种付款等待状态出现；逾期与重寄后等待付款必有期限。
fn valid_payment_deadline(lot: &Map<String, Value>, status: &str) -> bool {
    match lot.get("payment_deadline_at") {
        Some(deadline) => {
            matches!(status, "awaiting_payment" | "payment_overdue" | "cod_resent_awaiting_payment")
                && string(Some(deadline)).is_some_and(utc_second_timestamp)
        }
        None => !matches!(status, "payment_overdue" | "cod_resent_awaiting_payment"),
    }
}

/// 服务端只输出 `YYYY-MM-DDTHH:MM:SSZ`（UTC、秒精度）：逐字节核对形状，再校验日历日期与时分秒
/// 范围。不经浏览器 `Date.parse` 或 RFC 3339 的宽松写法（小数秒、时区偏移），宿主与 wasm 结果一致。
fn utc_second_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 20
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
        });
    if !shaped {
        return false;
    }
    // 形状已保证这些位置都是 ASCII 数字。
    let number = |start: usize, end: usize| {
        bytes[start..end].iter().fold(0_u32, |number, digit| number * 10 + u32::from(digit - b'0'))
    };
    value.get(..10).and_then(auction_time::parse_date).is_some()
        && number(11, 13) < 24
        && number(14, 16) < 60
        && number(17, 19) < 60
}

/// 退回次数与冻结上限成对出现：1 ≤ 次数 ≤ 上限 ≤ 10；重寄与达上限状态必有，流拍状态没有。
/// 待寄出 COD 与等待确认收款只属于未发生退回的首次付款：发生过退回后，付款阶段在服务端只会是
/// 退件、重寄、逾期或处理中，带退回次数的这两种状态是矛盾组合，整页失败关闭。
fn valid_cod_returns(lot: &Map<String, Value>, status: &str) -> bool {
    match (lot.get("cod_return_count"), lot.get("cod_return_limit")) {
        (None, None) => !matches!(
            status,
            "cod_resend_pending" | "cod_resent_awaiting_payment" | "cod_return_limit_reached"
        ),
        (Some(count), Some(limit)) => {
            let (Some(count), Some(limit)) = (int(Some(count)), int(Some(limit))) else {
                return false;
            };
            !unsold_status(status)
                && !matches!(status, "payment_mail_pending" | "awaiting_payment")
                && (1..=limit).contains(&count)
                && (1..=10).contains(&limit)
                && (status != "cod_return_limit_reached" || count == limit)
        }
        _ => false,
    }
}

fn unsold_status(status: &str) -> bool {
    status.starts_with("unsold_")
}

fn sold_status(status: &str) -> bool {
    !unsold_status(status) && status != "unknown"
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{FULFILLMENT_STATUSES, utc_second_timestamp, valid_fulfillment_page, valid_lot};

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    fn lot(status: &str) -> Value {
        json!({
            "lot_id": "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c",
            "lot_number": 3,
            "equipment_name": "Legendary Ethereal Staff",
            "equipment_category": "Staff",
            "seller_name": "seller",
            "status": status,
        })
    }

    fn sold(status: &str) -> Value {
        let mut lot = lot(status);
        lot["buyer_name"] = json!("buyer");
        lot["final_price"] = json!(1_500_000);
        lot
    }

    fn page(lots: Vec<Value>) -> Value {
        json!({
            "auction": { "id": AUCTION_ID, "world": "main", "number": 12 },
            "lots": lots,
            "page": 1,
            "page_size": 50,
            "total": 3,
        })
    }

    fn accepted(value: &Value) -> bool {
        valid_fulfillment_page(value, AUCTION_ID, "main", 1, 50)
    }

    #[test]
    fn statuses_are_a_closed_set_of_distinct_values() {
        let mut statuses = FULFILLMENT_STATUSES.to_vec();
        statuses.sort_unstable();
        statuses.dedup();
        assert_eq!(statuses.len(), 31);
        assert!(!valid_lot(&lot("withdrawn")), "闭集外的状态整页拒绝");
        assert!(!valid_lot(&lot("UNSOLD_RETURNED")));
    }

    #[test]
    fn typical_page_is_accepted() {
        let mut awaiting = sold("awaiting_payment");
        awaiting["payment_deadline_at"] = json!("2026-11-16T09:46:40Z");
        let mut resent = sold("cod_resent_awaiting_payment");
        resent["payment_deadline_at"] = json!("2026-11-16T09:46:40Z");
        resent["cod_return_count"] = json!(1);
        resent["cod_return_limit"] = json!(3);
        resent["lot_id"] = json!("0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2d");
        let mut unsold = lot("unsold_return_pending");
        unsold["lot_id"] = json!("0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2e");
        assert!(accepted(&page(vec![awaiting, resent, unsold])));
        // 历史成交缺 recovery 时等待付款可以没有期限；未知状态可带或不带成交事实。
        assert!(valid_lot(&sold("awaiting_payment")));
        assert!(valid_lot(&lot("unknown")));
        assert!(valid_lot(&sold("unknown")));
    }

    #[test]
    fn page_is_bound_to_the_request_identity() {
        let value = page(vec![lot("unsold_returned")]);
        assert!(accepted(&value));
        assert!(!valid_fulfillment_page(&value, AUCTION_ID, "isekai", 1, 50), "世界不符");
        assert!(!valid_fulfillment_page(&value, AUCTION_ID, "main", 2, 50), "页码不符");
        assert!(!valid_fulfillment_page(&value, AUCTION_ID, "main", 1, 20), "页大小不符");
        let other = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2f";
        assert!(!valid_fulfillment_page(&value, other, "main", 1, 50), "场次不符");
        let duplicated = page(vec![lot("unsold_returned"), lot("unsold_return_sent")]);
        assert!(!accepted(&duplicated), "同页 lot 重复");
        let mut short_total = page(vec![lot("unsold_returned")]);
        short_total["total"] = json!(0);
        assert!(!accepted(&short_total), "总数小于本页");
    }

    #[test]
    fn non_empty_pages_end_within_the_total() {
        let second_page = |lots: Vec<Value>, total: i64| {
            let mut value = page(lots);
            value["page"] = json!(2);
            value["total"] = json!(total);
            valid_fulfillment_page(&value, AUCTION_ID, "main", 2, 50)
        };
        // 第 2 页从第 51 件开始：总数只有 1 件时不可能还有一行。
        assert!(!second_page(vec![lot("unsold_returned")], 1), "offset 加本页行数超过总数");
        assert!(!second_page(vec![lot("unsold_returned")], 50));
        assert!(second_page(vec![lot("unsold_returned")], 51));
        // 越过末页的空页仍然有效，由分页器纠偏页码。
        assert!(second_page(Vec::new(), 1));
        assert!(second_page(Vec::new(), 0));
    }

    #[test]
    fn extra_keys_are_rejected_at_every_level() {
        let mut extra_page = page(Vec::new());
        extra_page["cursor"] = json!("x");
        assert!(!accepted(&extra_page));
        let mut extra_auction = page(Vec::new());
        extra_auction["auction"]["status"] = json!("settled");
        assert!(!accepted(&extra_auction));
        for key in ["delivery_id", "blocking_code", "proxy_mode", "obligation_id"] {
            let mut extra = sold("completed");
            extra[key] = json!("x");
            assert!(!valid_lot(&extra), "{key}");
        }
    }

    #[test]
    fn sale_fields_are_paired_and_follow_the_status() {
        assert!(!valid_lot(&lot("completed")), "成交状态缺成交事实");
        assert!(!valid_lot(&sold("unsold_returned")), "流拍状态不得带成交事实");
        let mut buyer_only = lot("completed");
        buyer_only["buyer_name"] = json!("buyer");
        assert!(!valid_lot(&buyer_only), "买家与成交价成对");
        let mut free = sold("completed");
        free["final_price"] = json!(0);
        assert!(!valid_lot(&free), "成交价为正");
    }

    #[test]
    fn payment_deadline_only_accompanies_payment_waits() {
        for status in ["awaiting_payment", "payment_overdue", "cod_resent_awaiting_payment"] {
            let mut waiting = sold(status);
            waiting["payment_deadline_at"] = json!("2026-11-16T09:46:40Z");
            if status == "cod_resent_awaiting_payment" {
                waiting["cod_return_count"] = json!(1);
                waiting["cod_return_limit"] = json!(3);
            }
            assert!(valid_lot(&waiting), "{status}");
        }
        assert!(!valid_lot(&sold("payment_overdue")), "逾期必有期限");
        let mut completed = sold("completed");
        completed["payment_deadline_at"] = json!("2026-11-16T09:46:40Z");
        assert!(!valid_lot(&completed), "其他状态不带期限");
        let mut malformed = sold("awaiting_payment");
        malformed["payment_deadline_at"] = json!("tomorrow");
        assert!(!valid_lot(&malformed));
        let mut offset = sold("awaiting_payment");
        offset["payment_deadline_at"] = json!("2026-11-16T17:46:40+08:00");
        assert!(!valid_lot(&offset), "期限只接受 UTC 秒精度");
    }

    #[test]
    fn payment_deadline_format_is_strict_utc_seconds() {
        for accepted in ["2026-11-16T09:46:40Z", "2028-02-29T23:59:59Z", "2026-01-01T00:00:00Z"] {
            assert!(utc_second_timestamp(accepted), "{accepted}");
        }
        for rejected in [
            "2026-11-16T09:46:40.123Z",
            "2026-11-16T09:46:40+00:00",
            "2026-11-16T09:46:40",
            "2026-11-16 09:46:40Z",
            "2026-11-16t09:46:40z",
            " 2026-11-16T09:46:40Z",
            "+2026-11-16T09:46:40Z",
            "2026-1-16T09:46:40Z",
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-00T00:00:00Z",
            "2026-11-16T24:00:00Z",
            "2026-11-16T09:60:00Z",
            "2026-11-16T09:46:60Z",
            "２０２６-11-16T09:46:40Z",
            "",
        ] {
            assert!(!utc_second_timestamp(rejected), "{rejected}");
        }
    }

    #[test]
    fn cod_returns_are_paired_and_bounded() {
        let with_returns = |status: &str, count: Value, limit: Value| {
            let mut lot = sold(status);
            lot["cod_return_count"] = count;
            lot["cod_return_limit"] = limit;
            lot
        };
        assert!(valid_lot(&with_returns("returned_by_buyer", json!(1), json!(3))));
        assert!(valid_lot(&with_returns("cod_return_limit_reached", json!(3), json!(3))));
        assert!(!valid_lot(&with_returns("cod_return_limit_reached", json!(2), json!(3))));
        assert!(!valid_lot(&with_returns("returned_by_buyer", json!(0), json!(3))));
        assert!(!valid_lot(&with_returns("returned_by_buyer", json!(4), json!(3))));
        assert!(!valid_lot(&with_returns("returned_by_buyer", json!(1), json!(11))));
        assert!(!valid_lot(&with_returns("returned_by_buyer", json!("1"), json!(3))));
        assert!(!valid_lot(&sold("cod_resend_pending")), "重寄状态必有退回次数");
        let mut count_only = sold("returned_by_buyer");
        count_only["cod_return_count"] = json!(1);
        assert!(!valid_lot(&count_only), "次数与上限成对");
        let mut unsold = lot("unsold_return_pending");
        unsold["cod_return_count"] = json!(1);
        unsold["cod_return_limit"] = json!(3);
        assert!(!valid_lot(&unsold), "流拍没有退回次数");
        // 发生过退回后不会再处于首次付款的两种状态：带退回次数即矛盾组合。
        let mut awaiting = with_returns("awaiting_payment", json!(1), json!(3));
        awaiting["payment_deadline_at"] = json!("2026-11-16T09:46:40Z");
        assert!(!valid_lot(&awaiting), "等待确认收款不带退回次数");
        assert!(!valid_lot(&with_returns("payment_mail_pending", json!(1), json!(3))));
        // 历史成交缺 recovery 时，等待确认收款可以既无期限也无退回次数。
        assert!(valid_lot(&sold("awaiting_payment")));
        assert!(valid_lot(&sold("payment_mail_pending")));
        // 发生过退回后仍可逾期或付款处理中，期间成对公开退回次数。
        let mut overdue = with_returns("payment_overdue", json!(1), json!(3));
        overdue["payment_deadline_at"] = json!("2026-11-16T09:46:40Z");
        assert!(valid_lot(&overdue));
        assert!(valid_lot(&with_returns("payment_on_hold", json!(2), json!(3))));
    }
}

//! 已脱敏 fixture 的编译期引用，拒绝未知页码和未捕获详情。

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../../auction-integrations/tests/fixtures/mooglemail/",
            $name,
            ".html"
        ))
    };
}

pub(super) const READ_MAIN: &str = fixture!("read-main");
pub(super) const MISSING_SENT: &str = fixture!("sentdetail-missing");
pub(super) const IDENTITY_MAIN: &str = fixture!("consign-identity-main");
pub(super) const IDENTITY_ISEKAI: &str = fixture!("consign-identity-isekai");

pub(super) fn page(
    world: &str,
    filter: &str,
    page: usize,
    mid: Option<&str>,
) -> Option<&'static str> {
    Some(match (world, filter, page, mid) {
        ("main", "inbox", 0, None) => fixture!("inbox-main"),
        ("isekai", "inbox", 0, None) => fixture!("inbox-isekai"),
        ("main", "read", 0, None) => READ_MAIN,
        ("main", "read", 1, None) => fixture!("read-page1"),
        ("main", "read", 2, None) => fixture!("read-page2"),
        ("isekai", "read", 0, None) => fixture!("read-isekai"),
        ("main", "sent", 0, None) => fixture!("sent-main"),
        ("isekai", "sent", 0, None) => fixture!("sent-isekai"),
        ("main", "new", 0, None) => fixture!("compose-main"),
        ("isekai", "new", 0, None) => fixture!("compose-isekai"),
        ("main", "inbox", 0, Some("3125357")) => fixture!("readdetail-inbox-3125357"),
        ("main", "read", 0, Some("3125357")) => fixture!("readdetail-read-3125357"),
        ("main", "inbox" | "read", 0, Some("3125358")) => fixture!("taken-3125358"),
        ("main", "sent", 0, Some("3125366")) => fixture!("sentdetail-3125366"),
        ("main", "sent", 0, Some("3125364")) => fixture!("sentdetail-3125364"),
        ("main", "sent", 0, Some("3125344")) => fixture!("sentdetail-3125344"),
        ("main", "sent", 0, Some("3125305")) => fixture!("sentdetail-3125305"),
        ("main", "sent", 0, Some("3125298")) => fixture!("sentdetail-3125298"),
        ("main", "sent", 0, Some("0")) => MISSING_SENT,
        ("main", "inbox" | "read", 0, Some("0")) => fixture!("inboxdetail-missing"),
        ("main", "inbox" | "read", 0, Some("900001")) => fixture!("consign-detail-main-single"),
        ("main", "inbox" | "read", 0, Some("900002")) => fixture!("consign-detail-main-multi"),
        ("main", "inbox" | "read", 0, Some("900004")) => {
            fixture!("consign-detail-main-duplicate-eid")
        }
        ("main", "inbox" | "read", 0, Some("900005")) => fixture!("consign-detail-main-alias"),
        ("main", "inbox" | "read", 0, Some("900006")) => fixture!("consign-detail-main-unknown"),
        ("main", "inbox" | "read", 0, Some("900007")) => fixture!("consign-detail-main-bad-count"),
        ("main", "inbox" | "read", 0, Some("900008")) => fixture!("consign-detail-main-credits"),
        ("main", "inbox" | "read", 0, Some("900009")) => fixture!("consign-detail-main-cod"),
        ("main", "inbox" | "read", 0, Some("900010")) => {
            fixture!("consign-detail-main-wrong-recipient")
        }
        ("main", "inbox" | "read", 0, Some("900011")) => {
            fixture!("consign-detail-main-wrong-world")
        }
        ("main", "inbox" | "read", 0, Some("900012")) => {
            fixture!("consign-detail-main-same-name")
        }
        ("main", "inbox" | "read", 0, Some("900013")) => {
            fixture!("consign-take-after-main-single")
        }
        ("main", "inbox" | "read", 0, Some("900014")) => {
            fixture!("consign-return-receipt-main")
        }
        ("isekai", "inbox" | "read", 0, Some("910001")) => {
            fixture!("consign-detail-isekai-multi")
        }
        _ => return None,
    })
}

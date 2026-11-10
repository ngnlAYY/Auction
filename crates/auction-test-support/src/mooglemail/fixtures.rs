//! 已脱敏 fixture 的编译期引用，拒绝未知页码和未捕获详情。

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../../auction-integrations/tests/fixtures/w6_3/", $name, ".html"))
    };
}

pub(super) const READ_MAIN: &str = fixture!("read-main");
pub(super) const MISSING_SENT: &str = fixture!("sentdetail-missing");

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
        _ => return None,
    })
}

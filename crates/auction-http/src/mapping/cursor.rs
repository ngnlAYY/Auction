//! Public 目录、竞价日志、价格历史、补偿池与 Admin 代理历史的不透明 keyset 游标编解码。
//! 五种游标共用 v1 定长帧：`[版本=1][类型][定长正文]` 经无填充 base64url 编码，字面量 `start`
//! 表示从第一条开始。帧已发给客户端，字节布局的任何变化只能以新的类型或版本表达。

use auction_application::admin::AdminProxyEventCursor;
use auction_application::auction::{
    PublicAuctionCursor, PublicBidLogCursor, PublicPriceHistoryCursor,
};
use auction_application::public::PublicCompensationCursor;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::DateTime;
use uuid::Uuid;

/// 表示从第一条开始的哨兵游标。
const START_CURSOR: &str = "start";

/// 帧首字节的格式版本。
const FRAME_VERSION: u8 = 1;

/// 帧第二字节的已发布游标类型；显式判别值由编译器保证互不重复，各端点只接受自己的类型。
#[derive(Clone, Copy)]
#[repr(u8)]
enum CursorKind {
    PublicAuction = 3,
    PublicBidLog = 4,
    PublicPriceHistory = 5,
    PublicCompensation = 6,
    AdminProxyEvent = 7,
}

/// 写入 `[版本][类型]` 帧头，由 `fill` 从第 2 字节起按 wire 偏移写入正文，再无填充编码。
fn encode_frame<const N: usize>(kind: CursorKind, fill: impl FnOnce(&mut [u8; N])) -> String {
    let mut frame = [0_u8; N];
    frame[0] = FRAME_VERSION;
    frame[1] = kind as u8;
    fill(&mut frame);
    URL_SAFE_NO_PAD.encode(frame)
}

/// 严格解码 `N` 字节的 v1 帧：输入必须恰为 `N` 字节的无填充编码长度，字母表、规范尾位、版本
/// 与类型任一不符即拒绝。先比较长度再解码到栈上定长缓冲，畸形输入不分配堆内存。
fn decode_frame<const N: usize>(raw: &str, kind: CursorKind) -> Option<[u8; N]> {
    if raw.len() != (N * 4).div_ceil(3) {
        return None;
    }
    let mut frame = [0_u8; N];
    // 严格引擎拒绝填充与非规范尾位；解码错误细节对客户端统一折叠为无效游标，无需保留。
    let Ok(decoded_len) = URL_SAFE_NO_PAD.decode_slice(raw, &mut frame) else {
        return None;
    };
    (decoded_len == N && frame.starts_with(&[FRAME_VERSION, kind as u8])).then_some(frame)
}

/// 处理 `start` 哨兵，其余输入交给 `boundary` 解析帧与字段；任何拒绝都折叠为同一个 `Err(())`。
fn decode_cursor<T>(
    raw: &str,
    start: T,
    boundary: impl FnOnce(&str) -> Option<T>,
) -> Result<T, ()> {
    if raw == START_CURSOR { Ok(start) } else { boundary(raw).ok_or(()) }
}

/// 读取帧内 `at` 偏移处的大端 i64。
fn be_i64(frame: &[u8], at: usize) -> Option<i64> {
    frame.get(at..)?.first_chunk().copied().map(i64::from_be_bytes)
}

/// 读取帧内 `at` 偏移处的大端 i32。
fn be_i32(frame: &[u8], at: usize) -> Option<i32> {
    frame.get(at..)?.first_chunk().copied().map(i32::from_be_bytes)
}

/// 读取帧内 `at` 偏移处的 16 字节 UUID。
fn uuid_at(frame: &[u8], at: usize) -> Option<Uuid> {
    frame.get(at..)?.first_chunk().copied().map(Uuid::from_bytes)
}

/// 编码拍卖目录 v1/type3 游标，26 字节：帧头、i64 序号、UUID。
pub(crate) fn encode_public_cursor(cursor: &PublicAuctionCursor) -> String {
    match cursor {
        PublicAuctionCursor::Start => START_CURSOR.to_owned(),
        PublicAuctionCursor::Boundary { sequence, id } => {
            encode_frame::<26>(CursorKind::PublicAuction, |frame| {
                frame[2..10].copy_from_slice(&sequence.to_be_bytes());
                frame[10..].copy_from_slice(id.as_bytes());
            })
        }
    }
}

/// 严格解码拍卖目录游标；帧、序号或 ID 不合法时拒绝。
pub(crate) fn decode_public_cursor(raw: &str) -> Result<PublicAuctionCursor, ()> {
    decode_cursor(raw, PublicAuctionCursor::Start, |raw| {
        let frame = decode_frame::<26>(raw, CursorKind::PublicAuction)?;
        let sequence = be_i64(&frame, 2)?;
        let id = uuid_at(&frame, 10)?;
        if sequence <= 0 || id.is_nil() {
            return None;
        }
        Some(PublicAuctionCursor::Boundary { sequence, id })
    })
}

/// 编码竞价日志 v1/type4 游标，54 字节：帧头、i64 微秒时间、lot UUID、i64 状态版本、
/// i32 事实序号、UUID。
pub(crate) fn encode_public_bid_log_cursor(cursor: &PublicBidLogCursor) -> String {
    match cursor {
        PublicBidLogCursor::Start => START_CURSOR.to_owned(),
        PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        } => encode_frame::<54>(CursorKind::PublicBidLog, |frame| {
            frame[2..10].copy_from_slice(&created_at.timestamp_micros().to_be_bytes());
            frame[10..26].copy_from_slice(lot_id.as_bytes());
            frame[26..34].copy_from_slice(&accepted_state_version.to_be_bytes());
            frame[34..38].copy_from_slice(&fact_index.to_be_bytes());
            frame[38..54].copy_from_slice(id.as_bytes());
        }),
    }
}

/// 严格解码竞价日志游标；帧、时间、lot、版本、事实序号或 ID 不合法时拒绝。
pub(crate) fn decode_public_bid_log_cursor(raw: &str) -> Result<PublicBidLogCursor, ()> {
    decode_cursor(raw, PublicBidLogCursor::Start, |raw| {
        let frame = decode_frame::<54>(raw, CursorKind::PublicBidLog)?;
        let timestamp_micros = be_i64(&frame, 2)?;
        let created_at = DateTime::from_timestamp_micros(timestamp_micros)?;
        let lot_id = uuid_at(&frame, 10)?;
        let accepted_state_version = be_i64(&frame, 26)?;
        let fact_index = be_i32(&frame, 34)?;
        let id = uuid_at(&frame, 38)?;
        if timestamp_micros == 0
            || lot_id.is_nil()
            || accepted_state_version <= 0
            || fact_index <= 0
            || id.is_nil()
        {
            return None;
        }
        Some(PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        })
    })
}

/// 编码价格历史 v1/type5 游标，38 字节：帧头、i64 微秒时间、i64 场次序号、i32 lot 编号、
/// lot UUID。
pub(crate) fn encode_public_price_history_cursor(cursor: &PublicPriceHistoryCursor) -> String {
    // v1/type5 的词法只表达 closed_desc，其他排序在 HTTP 与应用请求校验中拒绝 cursor。
    match cursor {
        PublicPriceHistoryCursor::Start => START_CURSOR.to_owned(),
        PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id } => {
            encode_frame::<38>(CursorKind::PublicPriceHistory, |frame| {
                frame[2..10].copy_from_slice(&closed_at.timestamp_micros().to_be_bytes());
                frame[10..18].copy_from_slice(&auction_sequence.to_be_bytes());
                frame[18..22].copy_from_slice(&(*lot_number as i32).to_be_bytes());
                frame[22..38].copy_from_slice(lot_id.as_bytes());
            })
        }
    }
}

/// 严格解码价格历史游标；帧、时间、场次、lot 编号或 lot ID 不合法时拒绝。
pub(crate) fn decode_public_price_history_cursor(
    raw: &str,
) -> Result<PublicPriceHistoryCursor, ()> {
    decode_cursor(raw, PublicPriceHistoryCursor::Start, |raw| {
        let frame = decode_frame::<38>(raw, CursorKind::PublicPriceHistory)?;
        let timestamp_micros = be_i64(&frame, 2)?;
        let closed_at = DateTime::from_timestamp_micros(timestamp_micros)?;
        let auction_sequence = be_i64(&frame, 10)?;
        let lot_number = i64::from(be_i32(&frame, 18)?);
        let lot_id = uuid_at(&frame, 22)?;
        if timestamp_micros == 0 || auction_sequence <= 0 || lot_number <= 0 || lot_id.is_nil() {
            return None;
        }
        Some(PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id })
    })
}

/// 编码补偿池 v1/type6 游标，26 字节：帧头、i64 拍卖序号、拍卖 UUID。
pub(crate) fn encode_public_compensation_cursor(cursor: &PublicCompensationCursor) -> String {
    match cursor {
        PublicCompensationCursor::Start => START_CURSOR.to_owned(),
        PublicCompensationCursor::Boundary { sequence, id } => {
            encode_frame::<26>(CursorKind::PublicCompensation, |frame| {
                frame[2..10].copy_from_slice(&sequence.to_be_bytes());
                frame[10..].copy_from_slice(id.as_bytes());
            })
        }
    }
}

/// 严格解码补偿池游标；帧、序号或 ID 不合法时拒绝。
pub(crate) fn decode_public_compensation_cursor(raw: &str) -> Result<PublicCompensationCursor, ()> {
    decode_cursor(raw, PublicCompensationCursor::Start, |raw| {
        let frame = decode_frame::<26>(raw, CursorKind::PublicCompensation)?;
        let sequence = be_i64(&frame, 2)?;
        let id = uuid_at(&frame, 10)?;
        if sequence <= 0 || id.is_nil() {
            return None;
        }
        Some(PublicCompensationCursor::Boundary { sequence, id })
    })
}

/// 编码 Admin 代理历史 v1/type7 游标，26 字节：帧头、i64 微秒时间、命令 UUID。
pub(crate) fn encode_admin_proxy_event_cursor(cursor: AdminProxyEventCursor) -> String {
    match cursor {
        AdminProxyEventCursor::Start => START_CURSOR.to_owned(),
        AdminProxyEventCursor::Boundary { created_at, command_id } => {
            encode_frame::<26>(CursorKind::AdminProxyEvent, |frame| {
                frame[2..10].copy_from_slice(&created_at.timestamp_micros().to_be_bytes());
                frame[10..].copy_from_slice(command_id.as_bytes());
            })
        }
    }
}

/// 严格解码 Admin 代理历史游标；帧不合法或边界未通过应用端口的校验时拒绝。
pub(crate) fn decode_admin_proxy_event_cursor(raw: &str) -> Result<AdminProxyEventCursor, ()> {
    decode_cursor(raw, AdminProxyEventCursor::Start, |raw| {
        let frame = decode_frame::<26>(raw, CursorKind::AdminProxyEvent)?;
        let created_at = DateTime::from_timestamp_micros(be_i64(&frame, 2)?)?;
        let command_id = uuid_at(&frame, 10)?;
        let cursor = AdminProxyEventCursor::Boundary { created_at, command_id };
        cursor.validate().then_some(cursor)
    })
}

#[cfg(test)]
mod tests {
    use auction_application::admin::AdminProxyEventCursor;
    use auction_application::auction::{
        PublicAuctionCursor, PublicBidLogCursor, PublicPriceHistoryCursor,
    };
    use auction_application::public::PublicCompensationCursor;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use chrono::{DateTime, Utc};
    use uuid::Uuid;

    use super::{
        decode_admin_proxy_event_cursor, decode_public_bid_log_cursor,
        decode_public_compensation_cursor, decode_public_cursor,
        decode_public_price_history_cursor, encode_admin_proxy_event_cursor,
        encode_public_bid_log_cursor, encode_public_compensation_cursor, encode_public_cursor,
        encode_public_price_history_cursor,
    };

    // 金标串按 `[版本][类型][正文]` 字节布局独立计算；游标已发给客户端，任何一个字节变化都是
    // wire 破坏。
    const AUCTION_WIRE: &str = "AQMBAgMEBQYHCAARIjNEVWZ3iJmqu8zd7v8";
    const BID_LOG_WIRE: &str =
        "AQQABgokGCAiQAARIjNEVWZ3iJmqu8zd7v8AAAAAAAAAKgAAAAf_7t3Mu6qZiHdmVUQzIhEA";
    const PRICE_HISTORY_WIRE: &str = "AQUABgokGCg78QAAAAAAAAAMAAAAAwEjRWeJq83v_ty6mHZUMhA";
    const COMPENSATION_WIRE: &str = "AQYBAgMEBQYHCAARIjNEVWZ3iJmqu8zd7v8";
    const PROXY_EVENT_WIRE: &str = "AQcABgokGCAiQP_u3cy7qpmId2ZVRDMiEQA";
    const URL_SAFE_ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    /// 一种游标的解码入口与金标串；拒绝用例对五种游标统一断言。
    struct WireCase {
        name: &'static str,
        kind: u8,
        golden: &'static str,
        accepts: fn(&str) -> bool,
    }

    fn cases() -> [WireCase; 5] {
        [
            WireCase {
                name: "auction",
                kind: 3,
                golden: AUCTION_WIRE,
                accepts: |raw| decode_public_cursor(raw).is_ok(),
            },
            WireCase {
                name: "bid_log",
                kind: 4,
                golden: BID_LOG_WIRE,
                accepts: |raw| decode_public_bid_log_cursor(raw).is_ok(),
            },
            WireCase {
                name: "price_history",
                kind: 5,
                golden: PRICE_HISTORY_WIRE,
                accepts: |raw| decode_public_price_history_cursor(raw).is_ok(),
            },
            WireCase {
                name: "compensation",
                kind: 6,
                golden: COMPENSATION_WIRE,
                accepts: |raw| decode_public_compensation_cursor(raw).is_ok(),
            },
            WireCase {
                name: "proxy_event",
                kind: 7,
                golden: PROXY_EVENT_WIRE,
                accepts: |raw| decode_admin_proxy_event_cursor(raw).is_ok(),
            },
        ]
    }

    fn first_id() -> Uuid {
        Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }

    fn second_id() -> Uuid {
        Uuid::from_u128(0xffee_ddcc_bbaa_9988_7766_5544_3322_1100)
    }

    fn third_id() -> Uuid {
        Uuid::from_u128(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210)
    }

    fn at_micros(micros: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_micros(micros).expect("测试时间在 chrono 范围内")
    }

    /// 把金标帧 `at` 起的字节改写为 `bytes` 后重新规范编码，构造长度合法但字段非法的游标。
    fn patched(raw: &str, at: usize, bytes: &[u8]) -> String {
        let mut frame = URL_SAFE_NO_PAD.decode(raw).expect("金标串是合法 base64url");
        frame[at..at + bytes.len()].copy_from_slice(bytes);
        URL_SAFE_NO_PAD.encode(frame)
    }

    /// 置位末字符承载的零填充位：解码出的数据不变，但不再是规范编码。
    fn with_dirty_trailing_bits(raw: &str) -> String {
        let (head, last) = raw.split_at(raw.len() - 1);
        let index = URL_SAFE_ALPHABET
            .iter()
            .position(|symbol| last.as_bytes() == [*symbol])
            .expect("末字符属于 URL 安全字母表");
        format!("{head}{}", char::from(URL_SAFE_ALPHABET[index | 1]))
    }

    #[test]
    fn boundary_cursors_match_golden_wire_and_round_trip() {
        let auction =
            PublicAuctionCursor::Boundary { sequence: 0x0102_0304_0506_0708, id: first_id() };
        assert_eq!(encode_public_cursor(&auction), AUCTION_WIRE);
        assert_eq!(decode_public_cursor(AUCTION_WIRE), Ok(auction));

        let bid_log = PublicBidLogCursor::Boundary {
            created_at: at_micros(1_700_000_000_123_456),
            lot_id: first_id(),
            accepted_state_version: 42,
            fact_index: 7,
            id: second_id(),
        };
        assert_eq!(encode_public_bid_log_cursor(&bid_log), BID_LOG_WIRE);
        assert_eq!(decode_public_bid_log_cursor(BID_LOG_WIRE), Ok(bid_log));

        let price_history = PublicPriceHistoryCursor::Boundary {
            closed_at: at_micros(1_700_000_000_654_321),
            auction_sequence: 12,
            lot_number: 3,
            lot_id: third_id(),
        };
        assert_eq!(encode_public_price_history_cursor(&price_history), PRICE_HISTORY_WIRE);
        assert_eq!(decode_public_price_history_cursor(PRICE_HISTORY_WIRE), Ok(price_history));

        let compensation =
            PublicCompensationCursor::Boundary { sequence: 0x0102_0304_0506_0708, id: first_id() };
        assert_eq!(encode_public_compensation_cursor(&compensation), COMPENSATION_WIRE);
        assert_eq!(decode_public_compensation_cursor(COMPENSATION_WIRE), Ok(compensation));

        let proxy_event = AdminProxyEventCursor::Boundary {
            created_at: at_micros(1_700_000_000_123_456),
            command_id: second_id(),
        };
        assert_eq!(encode_admin_proxy_event_cursor(proxy_event), PROXY_EVENT_WIRE);
        assert_eq!(decode_admin_proxy_event_cursor(PROXY_EVENT_WIRE), Ok(proxy_event));
    }

    #[test]
    fn start_sentinel_is_exact_for_every_kind() {
        assert_eq!(encode_public_cursor(&PublicAuctionCursor::Start), "start");
        assert_eq!(decode_public_cursor("start"), Ok(PublicAuctionCursor::Start));
        assert_eq!(encode_public_bid_log_cursor(&PublicBidLogCursor::Start), "start");
        assert_eq!(decode_public_bid_log_cursor("start"), Ok(PublicBidLogCursor::Start));
        assert_eq!(encode_public_price_history_cursor(&PublicPriceHistoryCursor::Start), "start");
        assert_eq!(
            decode_public_price_history_cursor("start"),
            Ok(PublicPriceHistoryCursor::Start)
        );
        assert_eq!(encode_public_compensation_cursor(&PublicCompensationCursor::Start), "start");
        assert_eq!(decode_public_compensation_cursor("start"), Ok(PublicCompensationCursor::Start));
        assert_eq!(encode_admin_proxy_event_cursor(AdminProxyEventCursor::Start), "start");
        assert_eq!(decode_admin_proxy_event_cursor("start"), Ok(AdminProxyEventCursor::Start));

        for case in cases() {
            for raw in ["", "Start", "START", " start", "start ", "start=", "c3RhcnQ"] {
                assert!(!(case.accepts)(raw), "{} 不得接受近似哨兵 {raw:?}", case.name);
            }
        }
    }

    #[test]
    fn decoders_reject_foreign_versions_and_kinds() {
        for case in cases() {
            for version in [0_u8, 2, u8::MAX] {
                let raw = patched(case.golden, 0, &[version]);
                assert!(!(case.accepts)(&raw), "{} 不得接受版本 {version}", case.name);
            }
            for kind in (0_u8..=8).chain([u8::MAX]).filter(|kind| *kind != case.kind) {
                let raw = patched(case.golden, 1, &[kind]);
                assert!(!(case.accepts)(&raw), "{} 不得接受类型 {kind}", case.name);
            }
            // 其他类型的游标一律互不接受（同长度的只差类型字节）。
            for other in cases().iter().filter(|other| other.kind != case.kind) {
                assert!(
                    !(case.accepts)(other.golden),
                    "{} 不得接受 {} 的游标",
                    case.name,
                    other.name
                );
            }
        }
    }

    #[test]
    fn decoders_reject_malformed_lengths_and_alphabet() {
        for case in cases() {
            let golden = case.golden;
            let without_last = &golden[..golden.len() - 1];
            let malformed = [
                without_last.to_owned(),
                format!("{golden}A"),
                format!("{golden}="),
                format!("{without_last}="),
                format!("+{}", &golden[1..]),
                format!("/{}", &golden[1..]),
                format!(" {without_last}"),
                golden.repeat(2),
            ];
            for raw in malformed {
                assert!(!(case.accepts)(&raw), "{} 不得接受畸形游标 {raw:?}", case.name);
            }
        }
    }

    #[test]
    fn decoders_reject_non_canonical_trailing_bits() {
        // 54 字节的竞价日志恰好编码为 72 个字符，没有填充位；其余四种帧末字符各带 2 个零位。
        for case in cases().into_iter().filter(|case| case.kind != 4) {
            let raw = with_dirty_trailing_bits(case.golden);
            assert_ne!(raw, case.golden);
            assert!(!(case.accepts)(&raw), "{} 不得接受非规范尾位 {raw:?}", case.name);
        }
    }

    #[test]
    fn decoders_reject_invalid_boundary_fields() {
        let zero_i64 = 0_i64.to_be_bytes();
        let negative_i64 = (-1_i64).to_be_bytes();
        let zero_i32 = 0_i32.to_be_bytes();
        let negative_i32 = (-1_i32).to_be_bytes();
        let nil = [0_u8; 16];
        // i64 微秒的两端都超出 chrono 可表示范围。
        let beyond_max = i64::MAX.to_be_bytes();
        let beyond_min = i64::MIN.to_be_bytes();

        for (at, bytes) in [(2, &zero_i64[..]), (2, &negative_i64), (10, &nil)] {
            assert!(decode_public_cursor(&patched(AUCTION_WIRE, at, bytes)).is_err());
            assert!(
                decode_public_compensation_cursor(&patched(COMPENSATION_WIRE, at, bytes)).is_err()
            );
        }
        for (at, bytes) in [
            (2, &zero_i64[..]),
            (2, &beyond_max),
            (2, &beyond_min),
            (10, &nil),
            (26, &zero_i64),
            (26, &negative_i64),
            (34, &zero_i32),
            (34, &negative_i32),
            (38, &nil),
        ] {
            assert!(decode_public_bid_log_cursor(&patched(BID_LOG_WIRE, at, bytes)).is_err());
        }
        for (at, bytes) in [
            (2, &zero_i64[..]),
            (2, &beyond_max),
            (2, &beyond_min),
            (10, &zero_i64),
            (10, &negative_i64),
            (18, &zero_i32),
            (18, &negative_i32),
            (22, &nil),
        ] {
            assert!(
                decode_public_price_history_cursor(&patched(PRICE_HISTORY_WIRE, at, bytes))
                    .is_err()
            );
        }
        for (at, bytes) in [(2, &zero_i64[..]), (2, &beyond_max), (2, &beyond_min), (10, &nil)] {
            assert!(
                decode_admin_proxy_event_cursor(&patched(PROXY_EVENT_WIRE, at, bytes)).is_err()
            );
        }
    }

    #[test]
    fn decoders_accept_minimal_positive_fields() {
        let one_i64 = 1_i64.to_be_bytes();
        let one_i32 = 1_i32.to_be_bytes();
        let negative_epoch = (-1_i64).to_be_bytes();

        assert_eq!(
            decode_public_cursor(&patched(AUCTION_WIRE, 2, &one_i64)),
            Ok(PublicAuctionCursor::Boundary { sequence: 1, id: first_id() })
        );
        assert_eq!(
            decode_public_compensation_cursor(&patched(COMPENSATION_WIRE, 2, &one_i64)),
            Ok(PublicCompensationCursor::Boundary { sequence: 1, id: first_id() })
        );
        // 纪元前的非零时间是合法边界；只有恰为 0 的时间被当作缺失值拒绝。
        let bid_log = patched(
            &patched(&patched(BID_LOG_WIRE, 2, &negative_epoch), 26, &one_i64),
            34,
            &one_i32,
        );
        assert_eq!(
            decode_public_bid_log_cursor(&bid_log),
            Ok(PublicBidLogCursor::Boundary {
                created_at: at_micros(-1),
                lot_id: first_id(),
                accepted_state_version: 1,
                fact_index: 1,
                id: second_id(),
            })
        );
        let price_history = patched(
            &patched(&patched(PRICE_HISTORY_WIRE, 2, &negative_epoch), 10, &one_i64),
            18,
            &one_i32,
        );
        assert_eq!(
            decode_public_price_history_cursor(&price_history),
            Ok(PublicPriceHistoryCursor::Boundary {
                closed_at: at_micros(-1),
                auction_sequence: 1,
                lot_number: 1,
                lot_id: third_id(),
            })
        );
        assert_eq!(
            decode_admin_proxy_event_cursor(&patched(PROXY_EVENT_WIRE, 2, &negative_epoch)),
            Ok(AdminProxyEventCursor::Boundary {
                created_at: at_micros(-1),
                command_id: second_id()
            })
        );
    }
}

//! Public 目录、竞价日志与价格历史的不透明 keyset 游标编解码。

use auction_application::auction::{
    PublicAuctionCursor, PublicBidLogCursor, PublicPriceHistoryCursor,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::DateTime;
use uuid::Uuid;

/// 编码拍卖目录 v1/type3 游标；`start` 表示从第一条开始。
pub(crate) fn encode_public_cursor(cursor: &PublicAuctionCursor) -> String {
    match cursor {
        PublicAuctionCursor::Start => "start".to_owned(),
        PublicAuctionCursor::Boundary { sequence, id } => {
            let mut payload = [0_u8; 26];
            payload[0] = 1;
            payload[1] = 3;
            payload[2..10].copy_from_slice(&sequence.to_be_bytes());
            payload[10..].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

/// 严格解码拍卖目录游标；长度、版本、类型、序号或 ID 不合法时拒绝。
pub(crate) fn decode_public_cursor(raw: &str) -> Result<PublicAuctionCursor, ()> {
    if raw == "start" {
        return Ok(PublicAuctionCursor::Start);
    }
    // 固定 26 字节只接受 35 个无填充字符；严格引擎已拒绝非规范尾位，无需重编码分配。
    if raw.len() != 35 {
        return Err(());
    }
    let mut decoded = [0_u8; 26];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 3 {
        return Err(());
    }
    let sequence = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[10..]).map_err(|_| ())?;
    if sequence <= 0 || id.is_nil() {
        return Err(());
    }
    Ok(PublicAuctionCursor::Boundary { sequence, id })
}

/// 编码竞价日志 v1/type4 游标。
pub(crate) fn encode_public_bid_log_cursor(cursor: &PublicBidLogCursor) -> String {
    match cursor {
        PublicBidLogCursor::Start => "start".to_owned(),
        PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        } => {
            let mut payload = [0_u8; 54];
            payload[0] = 1;
            payload[1] = 4;
            payload[2..10].copy_from_slice(&created_at.timestamp_micros().to_be_bytes());
            payload[10..26].copy_from_slice(lot_id.as_bytes());
            payload[26..34].copy_from_slice(&accepted_state_version.to_be_bytes());
            payload[34..38].copy_from_slice(&fact_index.to_be_bytes());
            payload[38..54].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

/// 严格解码竞价日志游标；时间、lot、版本、事实序号或 ID 不合法时拒绝。
pub(crate) fn decode_public_bid_log_cursor(raw: &str) -> Result<PublicBidLogCursor, ()> {
    if raw == "start" {
        return Ok(PublicBidLogCursor::Start);
    }
    // v1/type4 固定 54 字节；与目录游标相同，直接解码到栈并保留严格字母表/尾位校验。
    if raw.len() != 72 {
        return Err(());
    }
    let mut decoded = [0_u8; 54];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 4 {
        return Err(());
    }
    let timestamp_micros = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let created_at = DateTime::from_timestamp_micros(timestamp_micros).ok_or(())?;
    let lot_id = Uuid::from_slice(&decoded[10..26]).map_err(|_| ())?;
    let accepted_state_version = i64::from_be_bytes(decoded[26..34].try_into().map_err(|_| ())?);
    let fact_index = i32::from_be_bytes(decoded[34..38].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[38..54]).map_err(|_| ())?;
    if timestamp_micros == 0
        || lot_id.is_nil()
        || accepted_state_version <= 0
        || fact_index <= 0
        || id.is_nil()
    {
        return Err(());
    }
    Ok(PublicBidLogCursor::Boundary { created_at, lot_id, accepted_state_version, fact_index, id })
}

/// 编码价格历史 v1/type5 游标。
pub(crate) fn encode_public_price_history_cursor(cursor: &PublicPriceHistoryCursor) -> String {
    // v1/type5 的词法只表达 closed_desc，其他排序在 HTTP 与应用请求校验中拒绝 cursor。
    match cursor {
        PublicPriceHistoryCursor::Start => "start".to_owned(),
        PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id } => {
            let mut payload = [0_u8; 38];
            payload[0] = 1;
            payload[1] = 5;
            payload[2..10].copy_from_slice(&closed_at.timestamp_micros().to_be_bytes());
            payload[10..18].copy_from_slice(&auction_sequence.to_be_bytes());
            payload[18..22].copy_from_slice(&(*lot_number as i32).to_be_bytes());
            payload[22..38].copy_from_slice(lot_id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

/// 严格解码价格历史游标；时间、场次、lot 编号或 lot ID 不合法时拒绝。
pub(crate) fn decode_public_price_history_cursor(
    raw: &str,
) -> Result<PublicPriceHistoryCursor, ()> {
    if raw == "start" {
        return Ok(PublicPriceHistoryCursor::Start);
    }
    // v1/type5 固定 38 字节，对应 51 个无填充字符；不为无效长度或规范性复核分配堆内存。
    if raw.len() != 51 {
        return Err(());
    }
    let mut decoded = [0_u8; 38];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 5 {
        return Err(());
    }
    let timestamp_micros = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let closed_at = DateTime::from_timestamp_micros(timestamp_micros).ok_or(())?;
    let auction_sequence = i64::from_be_bytes(decoded[10..18].try_into().map_err(|_| ())?);
    let lot_number = i64::from(i32::from_be_bytes(decoded[18..22].try_into().map_err(|_| ())?));
    let lot_id = Uuid::from_slice(&decoded[22..38]).map_err(|_| ())?;
    if timestamp_micros == 0 || auction_sequence <= 0 || lot_number <= 0 || lot_id.is_nil() {
        return Err(());
    }
    Ok(PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id })
}

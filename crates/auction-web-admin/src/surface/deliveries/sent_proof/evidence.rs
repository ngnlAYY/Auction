//! 可被人工证明引用的证据与冻结发送尝试的选取。
//!
//! 只有维护流程写入的两类观察可被引用；既有确认与草稿丢弃不构成新证据。

use crate::api::{DeliveryAttempt, DeliveryDetail, DeliveryEvidence};

/// 人工证明可以引用的证据种类。
pub(super) const SELECTABLE_EVIDENCE_KINDS: [&str; 2] =
    ["mailbox_sweep", "sent_reference_sighting"];

/// 详情中可被人工证明引用的证据，保持服务端顺序。
pub(crate) fn selectable_evidence(detail: &DeliveryDetail) -> Vec<&DeliveryEvidence> {
    detail
        .evidence
        .iter()
        .filter(|item| SELECTABLE_EVIDENCE_KINDS.contains(&item.kind.as_str()))
        .collect()
}

/// 冻结的发送尝试按 fence 取最新一条；缺失或只有 Take 尝试时不得引用。
pub(super) fn latest_send_attempt(detail: &DeliveryDetail) -> Option<&DeliveryAttempt> {
    detail
        .attempts
        .iter()
        .filter(|attempt| attempt.operation == "send")
        .max_by_key(|attempt| attempt.fencing_version)
}

/// 解析服务端小写 hex 摘要；长度或字符不合规时返回 `None`，不做大小写兼容。
pub(super) fn decode_hex_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let (chunks, _) = value.as_bytes().as_chunks::<2>();
    let mut digest = [0_u8; 32];
    for (index, chunk) in chunks.iter().enumerate() {
        digest[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Some(digest)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

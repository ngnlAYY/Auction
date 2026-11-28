//! Take 响应页的 marker 与余额观察；只有完整回执才允许上层形成已证明结果。

use super::ConsignmentDetailObservation;
use super::ConsignmentListRow;
use super::TakeReceiptObservation;
use super::parse::parse_consignment_detail;
use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::parser::{
    canonical_mail_body, parse_removed_attachment_markers, take_cod_receipt_amount,
};

/// 解析 Take 成功响应；解析不清时所有证明位保持 false，由调用方转未知待证。
pub fn observe_take_receipt(
    before: &ConsignmentDetailObservation,
    document: &[u8],
    world: MoogleMailWorld,
) -> TakeReceiptObservation {
    let row = ConsignmentListRow {
        external_mail_id: before.external_mail_id.clone(),
        world,
        world_marker: before.world_marker,
        sender: before.sender.clone(),
        subject: before.subject.clone(),
        received_minute_raw: before.received_minute_raw.clone(),
        received_at: before.received_at,
    };
    let after = parse_consignment_detail(document, world, &row);
    let after_page_digest = after.manifest_digest();
    let observed = canonical_mail_body(&after.body);
    let base = canonical_mail_body(&before.body);
    let receipts = observed.strip_prefix(&base);
    let identity_body_matches = after.recipient == before.recipient
        && after.sender == before.sender
        && after.subject == before.subject
        && after.world_marker == Some(world)
        && after.list_world_marker == Some(world)
        && after.list_detail_consistent
        && receipts.is_some();
    let assets_empty = after.items.is_empty()
        && after.credits == Some(0)
        && after.cod == Some(0)
        && after.reported_attachment_count == Some(0);
    let assets_unavailable =
        assets_empty && after.actions_proven && !after.claimable && after.mail_state == Some(2);
    let receipts_present = receipts.is_some();
    let receipts = receipts.unwrap_or_default();
    // 第 4 期整封接受禁止含 Credits / COD 的寄件，Take 不会遇到现金；现金回执金额核对
    // 在第 6 期现金腿出现时补。当前仍核对 COD 回执的方向与金额，并拒绝普通现金回执。
    // 未观察到回执时所有证明位保持 false，不能把“空后缀”当作方向相符。
    let receipt_direction_matches = receipts_present
        && if before.cod.unwrap_or(0) > 0 {
            i64::try_from(before.cod.unwrap_or(0))
                .ok()
                .is_some_and(|expected| take_cod_receipt_amount(receipts) == Some(expected))
        } else {
            !receipts.contains("CoD Paid:")
        };
    let markers = parse_removed_attachment_markers(receipts, world).unwrap_or_default();
    TakeReceiptObservation {
        identity_body_matches,
        assets_empty,
        assets_unavailable,
        credits: after.credits.and_then(|value| i64::try_from(value).ok()).unwrap_or(-1),
        cod: after.cod.and_then(|value| i64::try_from(value).ok()).unwrap_or(-1),
        receipt_direction_matches,
        after_page_digest,
        markers,
    }
}

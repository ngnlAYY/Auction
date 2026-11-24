//! 观察清单与派生事实的 JSON 编解码；键集合与类型在数据库侧同样受 CHECK 约束。

use auction_domain::SettlementWorld;
use auction_domain::consignment::deposit::{
    EnvelopeItemKind, EnvelopeItemObservation, EnvelopeObservation, LegacyAuctionOwner,
    LegacyReturnEvidence, SenderIdentity,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{invariant, invariant_decoding};
use crate::database::consignment::commands::errors::WriteError;

/// 观察行保存的、无法由其他列表达的观察时事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DerivedFacts {
    pub(super) subject_is_consign: bool,
    pub(super) sender_identity: SenderIdentity,
    pub(super) list_detail_consistent: bool,
    pub(super) duplicate_raw_eid: bool,
    pub(super) canonical_alias_ambiguous: bool,
    pub(super) unknown_attachment: bool,
    pub(super) system_rts: bool,
    pub(super) legacy_auction_owner: Option<LegacyAuctionOwner>,
    pub(super) legacy_return_evidence: LegacyReturnEvidence,
    pub(super) received_minute_raw: String,
    pub(super) asset_count: Option<u32>,
    pub(super) items: Vec<DerivedItem>,
}

/// 单件由 Bot 读装备详情得到的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DerivedItem {
    pub(super) ordinal: u32,
    pub(super) world: SettlementWorld,
    pub(super) tradeable: bool,
    /// 观察时历史门结果；仅供审计与排障，决定和解除必须在锁内重算，不读取本值。
    pub(super) history_ok: bool,
}

/// 把领域观察写成 `c4_valid_manifest` 允许的类型化 JSON 数组。
pub(crate) fn manifest_json(observation: &EnvelopeObservation) -> Value {
    Value::Array(
        observation
            .items
            .iter()
            .map(|item| {
                json!({
                    "ordinal": item.ordinal,
                    "kind": item.kind.as_str(),
                    "name": item.display_name,
                    "raw_eid": item.raw_eid,
                    "canonical_eid": item.canonical_eid,
                    "amount": item.amount,
                    "protected_ref": item.protected_ref.map(|value| value.to_string()),
                })
            })
            .collect(),
    )
}

/// 解析清单 JSON；未知种类或越界值失败关闭。
pub(crate) fn parse_manifest(
    value: &Value,
    _world: SettlementWorld,
) -> Result<Vec<EnvelopeItemObservation>, WriteError> {
    let Some(items) = value.as_array() else {
        return Err(invariant("consignment_manifest_array"));
    };
    if items.len() > 200 {
        return Err(invariant("consignment_manifest_length"));
    }
    items
        .iter()
        .map(|item| {
            let ordinal = item
                .get("ordinal")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| invariant("consignment_manifest_ordinal"))?;
            let kind = match item.get("kind").and_then(Value::as_str) {
                Some("equipment") => EnvelopeItemKind::Equipment,
                Some("credits") => EnvelopeItemKind::Credits,
                Some("cod") => EnvelopeItemKind::Cod,
                Some("unknown") => EnvelopeItemKind::Unknown,
                _ => return Err(invariant("consignment_manifest_kind")),
            };
            let protected_ref = item
                .get("protected_ref")
                .and_then(Value::as_str)
                .map(Uuid::parse_str)
                .transpose()
                .map_err(invariant_decoding("consignment_manifest_reference"))?;
            Ok(EnvelopeItemObservation {
                ordinal,
                kind,
                display_name: item.get("name").and_then(Value::as_str).map(str::to_owned),
                raw_eid: item.get("raw_eid").and_then(Value::as_str).map(str::to_owned),
                canonical_eid: item.get("canonical_eid").and_then(Value::as_str).map(str::to_owned),
                amount: item.get("amount").and_then(Value::as_u64),
                world: None,
                seller_user_id: None,
                tradeable: false,
                history_ok: false,
                protected_ref,
            })
        })
        .collect()
}

/// 生成派生事实 JSON；调用方只传观察时已经确定的事实。
pub(super) fn derived_facts_json(
    observation: &EnvelopeObservation,
    world: SettlementWorld,
    received_minute_raw: &str,
    legacy_return_evidence: &LegacyReturnEvidence,
) -> Value {
    json!({
        "subject_is_consign": observation.subject_is_consign,
        "sender_identity": sender_identity_code(observation.sender_identity),
        "list_detail_consistent": observation.list_detail_consistent,
        "duplicate_raw_eid": observation.duplicate_raw_eid,
        "canonical_alias_ambiguous": observation.canonical_alias_ambiguous,
        "unknown_attachment": observation.unknown_attachment,
        "system_rts": observation.system_rts,
        "legacy_auction_owner": observation.legacy_auction_owner.map(LegacyAuctionOwner::as_str),
        "legacy_obligation_id": legacy_return_evidence.obligation_id.map(|value| value.to_string()),
        "legacy_subject": legacy_return_evidence.subject,
        "returned_from": legacy_return_evidence.returned_from,
        "body_digest": legacy_return_evidence.body_digest.map(hex_digest),
        "dispatch_reference": legacy_return_evidence.dispatch_reference,
        "received_minute_raw": received_minute_raw,
        "asset_count": observation.asset_count,
        "items": observation.items.iter().map(|item| json!({
            "ordinal": item.ordinal,
            "world": auction_domain::consignment::settlement_world_code(
                item.world.unwrap_or(world)),
            "tradeable": item.tradeable,
            "history_ok": item.history_ok,
        })).collect::<Vec<_>>(),
    })
}

/// 解析派生事实；键集合、类型与闭集码不合法时失败关闭。
pub(super) fn parse_derived_facts(value: &Value) -> Result<DerivedFacts, WriteError> {
    let object = value.as_object().ok_or_else(|| invariant("consignment_derived_object"))?;
    if !(10..=16).contains(&object.len()) {
        return Err(invariant("consignment_derived_keys"));
    }
    let boolean = |key: &'static str| {
        value
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| invariant("consignment_derived_boolean"))
    };
    let sender_identity = match value.get("sender_identity").and_then(Value::as_str) {
        Some("verified") => SenderIdentity::Verified,
        Some("same_name_different_uid") => SenderIdentity::SameNameDifferentUid,
        Some("unverified") => SenderIdentity::Unverified,
        _ => return Err(invariant("consignment_derived_sender")),
    };
    let legacy_auction_owner = match value.get("legacy_auction_owner") {
        Some(Value::Null) | None => None,
        Some(Value::String(text)) => Some(
            LegacyAuctionOwner::parse(text)
                .ok_or_else(|| invariant("consignment_derived_owner"))?,
        ),
        Some(_) => return Err(invariant("consignment_derived_owner")),
    };
    let legacy_obligation_id = match value.get("legacy_obligation_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(
            Uuid::parse_str(text)
                .ok()
                .filter(|value| !value.is_nil() && value.to_string() == *text)
                .ok_or_else(|| invariant("consignment_derived_legacy_obligation"))?,
        ),
        Some(_) => return Err(invariant("consignment_derived_legacy_obligation")),
    };
    let legacy_subject = match value.get("legacy_subject") {
        None | Some(Value::Null) => None,
        Some(Value::String(text))
            if legacy_obligation_id.is_some()
                && !text.is_empty()
                && text.len() <= 512
                && text.trim() == text
                && !text.chars().any(char::is_control) =>
        {
            Some(text.clone())
        }
        Some(_) => return Err(invariant("consignment_derived_legacy_subject")),
    };
    let legacy_returned_from = match value.get("returned_from") {
        None | Some(Value::Null) => None,
        Some(Value::String(text))
            if !text.is_empty()
                && text.len() <= 256
                && text.trim() == text
                && !text.chars().any(char::is_control) =>
        {
            Some(text.clone())
        }
        Some(_) => return Err(invariant("consignment_derived_returned_from")),
    };
    let legacy_body_digest = match value.get("body_digest") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(
            parse_hex_digest(text)
                .ok_or_else(|| invariant("consignment_derived_legacy_body_digest"))?,
        ),
        Some(_) => return Err(invariant("consignment_derived_legacy_body_digest")),
    };
    let legacy_dispatch_reference = match value.get("dispatch_reference") {
        None | Some(Value::Null) => None,
        Some(Value::String(text))
            if !text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control) =>
        {
            Some(text.clone())
        }
        Some(_) => return Err(invariant("consignment_derived_dispatch_reference")),
    };
    let received_minute_raw = value
        .get("received_minute_raw")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invariant("consignment_derived_minute"))?;
    let asset_count = match value.get("asset_count") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value <= 200)
            .map(Some)
            .ok_or_else(|| invariant("consignment_derived_asset_count"))?,
        Some(_) => return Err(invariant("consignment_derived_asset_count")),
    };
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| invariant("consignment_derived_items"))?
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(|| invariant("consignment_derived_item"))?;
            if object.len() != 4 {
                return Err(invariant("consignment_derived_item_keys"));
            }
            Ok(DerivedItem {
                ordinal: item
                    .get("ordinal")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| invariant("consignment_derived_item_ordinal"))?,
                world: item
                    .get("world")
                    .and_then(Value::as_str)
                    .and_then(auction_domain::consignment::parse_settlement_world)
                    .ok_or_else(|| invariant("consignment_derived_item_world"))?,
                tradeable: item
                    .get("tradeable")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| invariant("consignment_derived_item_tradeable"))?,
                history_ok: item
                    .get("history_ok")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| invariant("consignment_derived_item_history"))?,
            })
        })
        .collect::<Result<Vec<_>, WriteError>>()?;
    Ok(DerivedFacts {
        subject_is_consign: boolean("subject_is_consign")?,
        sender_identity,
        list_detail_consistent: boolean("list_detail_consistent")?,
        duplicate_raw_eid: boolean("duplicate_raw_eid")?,
        canonical_alias_ambiguous: boolean("canonical_alias_ambiguous")?,
        unknown_attachment: boolean("unknown_attachment")?,
        system_rts: boolean("system_rts")?,
        legacy_auction_owner,
        legacy_return_evidence: LegacyReturnEvidence {
            obligation_id: legacy_obligation_id,
            subject: legacy_subject,
            returned_from: legacy_returned_from,
            body_digest: legacy_body_digest,
            dispatch_reference: legacy_dispatch_reference,
        },
        received_minute_raw,
        asset_count,
        items,
    })
}

fn sender_identity_code(identity: SenderIdentity) -> &'static str {
    match identity {
        SenderIdentity::Verified => "verified",
        SenderIdentity::SameNameDifferentUid => "same_name_different_uid",
        SenderIdentity::Unverified => "unverified",
    }
}

fn hex_digest(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_hex_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        digest[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(digest)
}

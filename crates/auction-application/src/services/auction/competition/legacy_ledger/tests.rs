//! 冻结旧 wire 的定向回归：确定性 ID、`facts:null` 编码与机制名称一旦变化，历史账本将无法重放。

use auction_domain::{
    BidAmount, CommandId, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTransition,
    ProxyTransitionKind,
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

use super::bid::{LegacyAcceptedBidTransition, LegacyBidMechanism};
use super::ids::deterministic_id;
use super::proxy::LegacyProxyTransitionRecord;
use super::proxy_result::{deserialize_legacy_proxy_facts, serialize_legacy_proxy_facts};

#[test]
fn deterministic_id_matches_frozen_wire_vectors() {
    let command_id =
        CommandId::from_uuid(Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap());
    for (namespace, index, expected) in [
        ("auction-bid-fact", 1, "9d124e29-92f5-5b58-9689-680dfead85b4"),
        ("auction-bid-fact", 2, "3f93e9e3-01a5-5e68-85ba-bbacb001470f"),
        ("auction-proxy-authorization", 1, "020d732c-189f-50bb-ab30-9b00e41c03e5"),
    ] {
        let derived = deterministic_id(command_id, namespace, index);
        assert_eq!(derived, Uuid::parse_str(expected).unwrap(), "{namespace}#{index}");
        assert_eq!(derived.get_version_num(), 5);
        assert_eq!(derived.get_variant(), Variant::RFC4122);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LegacyProxyFacts {
    #[serde(
        serialize_with = "serialize_legacy_proxy_facts",
        deserialize_with = "deserialize_legacy_proxy_facts"
    )]
    facts: Vec<LegacyAcceptedBidTransition>,
}

#[test]
fn proxy_facts_keep_frozen_null_encoding() {
    let encoded = serde_json::to_string(&LegacyProxyFacts { facts: Vec::new() }).unwrap();
    assert_eq!(encoded, r#"{"facts":null}"#);
    let decoded: LegacyProxyFacts = serde_json::from_str(r#"{"facts":null}"#).unwrap();
    assert!(decoded.facts.is_empty());
}

#[test]
fn bid_mechanism_wire_names_are_frozen() {
    assert_eq!(serde_json::to_string(&LegacyBidMechanism::Ordinary).unwrap(), r#""ordinary""#);
    assert_eq!(serde_json::to_string(&LegacyBidMechanism::Proxy).unwrap(), r#""proxy""#);
}

#[test]
fn proxy_transition_state_version_must_fit_the_ledger_column() {
    let transition = |state_version| ProxyTransition {
        authorization_id: ProxyAuthorizationId::from_uuid(Uuid::from_u128(1)),
        command_id: CommandId::from_uuid(Uuid::from_u128(2)),
        kind: ProxyTransitionKind::Created,
        old_maximum: None,
        new_maximum: BidAmount::new(80_000).unwrap(),
        old_mode: None,
        new_mode: ProxyMode::Fast,
        old_status: None,
        new_status: ProxyStatus::Active,
        state_version,
        reason: None,
    };
    let created_at = Utc.with_ymd_and_hms(2026, 11, 14, 0, 0, 0).unwrap();
    let record = LegacyProxyTransitionRecord::from_domain(&transition(7), created_at).unwrap();
    assert_eq!((record.transition_number, record.resulting_state_version), (7, 7));
    // 超出 i64 的版本号不能饱和成伪造的 i64::MAX 写入账本，必须失败关闭。
    assert!(LegacyProxyTransitionRecord::from_domain(&transition(u64::MAX), created_at).is_err());
}

//! 冻结旧 wire 的定向回归：确定性 ID、`facts:null` 编码与机制名称一旦变化，历史账本将无法重放。

use auction_domain::CommandId;
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

use super::bid::{LegacyAcceptedBidTransition, LegacyBidMechanism};
use super::ids::deterministic_id;
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

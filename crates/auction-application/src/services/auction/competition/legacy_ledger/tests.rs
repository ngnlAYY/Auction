//! 冻结旧 wire 的定向回归：确定性 ID、`facts:null` 编码与机制名称一旦变化，历史账本将无法重放。
//! 另以领域规划器生成的代理/普通竞价计划验证 encode→replay 往返：同一命令重放必须得到与首次
//! 执行相同的公开事实与代理响应，篡改账本载荷必须失败关闭（不变量 1：同 ID 同请求返回原结果）。

use auction_domain::{
    AuctionRules, AuctionStatus, BidAmount, CommandId, CompetitionPlan, DEFAULT_START_PRICE,
    LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus,
    ProxyTransition, ProxyTransitionKind, UserId,
};
use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant};

use super::super::commands::CompetitionCommand;
use super::bid::{LegacyAcceptedBidTransition, LegacyBidMechanism};
use super::decode::replay;
use super::encode::encode;
use super::ids::deterministic_id;
use super::proxy::LegacyProxyTransitionRecord;
use super::proxy_result::{deserialize_legacy_proxy_facts, serialize_legacy_proxy_facts};
use crate::errors::ApplicationError;
use crate::ports::auction::{
    CompetitionCommandOutcome, CompetitionProxyCommandOutcome, CompetitionWireTimestamp,
};
use crate::ports::core::{
    CompetitionProxyTransitionRecord, CompetitionSnapshot, CompetitionTransactionError,
    ProxyUsageContext, RequestHash,
};
use crate::services::auction::{CreateProxyCommand, LowerProxyCommand, PlaceBidCommand};

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
    let created_at = Timestamp::from_ymd_hms(2026, 11, 14, 0, 0, 0).unwrap();
    let record = LegacyProxyTransitionRecord::from_domain(&transition(7), created_at).unwrap();
    assert_eq!((record.transition_number, record.resulting_state_version), (7, 7));
    // 超出 i64 的版本号不能饱和成伪造的 i64::MAX 写入账本，必须失败关闭。
    assert!(LegacyProxyTransitionRecord::from_domain(&transition(u64::MAX), created_at).is_err());
}

const QUOTA_LIMIT: u32 = 3;

fn at(hour: u32, minute: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 1, 1, hour, minute, 0).expect("测试时间必须存在")
}

fn amount(value: i64) -> BidAmount {
    BidAmount::new(value).expect("测试金额必须合法")
}

fn lot_state(lot_id: LotId, current: i64, leader: UserId) -> LotBidState {
    LotBidState {
        lot_id,
        start_price: amount(DEFAULT_START_PRICE),
        current_price: Some(amount(current)),
        leader_id: Some(leader),
        leader_proxy_last: false,
        material: false,
        effective_end_at: at(12, 0),
        state_version: 1,
        last_proxy_sequence: 1,
        slow_generation: 0,
        slow_authorization_id: None,
        slow_due_at: None,
    }
}

fn fast_proxy(lot_id: LotId, user_id: UserId, maximum: i64) -> ProxyAuthorization {
    ProxyAuthorization::new(
        ProxyAuthorizationId::new(),
        lot_id,
        user_id,
        CommandId::new(),
        1,
        1,
        amount(maximum),
        ProxyMode::Fast,
    )
    .expect("测试代理授权必须合法")
}

fn snapshot(state: LotBidState, active_proxy: Option<ProxyAuthorization>) -> CompetitionSnapshot {
    CompetitionSnapshot {
        rules: AuctionRules::default(),
        auction_status: AuctionStatus::Live,
        state,
        active_proxy,
    }
}

/// 按服务的同一顺序构造触发器、领域计划与旧账本载荷。
fn encode_new_command(
    command: &CompetitionCommand,
    snapshot: &CompetitionSnapshot,
    usage: Option<ProxyUsageContext>,
    previous: &[CompetitionProxyTransitionRecord],
) -> (CompetitionPlan, Vec<u8>, Option<CompetitionProxyCommandOutcome>) {
    let trigger = command.trigger(snapshot, at(10, 0), usage).expect("触发器必须合法");
    let plan =
        CompetitionPlan::plan(snapshot.rules, snapshot.state, snapshot.active_proxy, trigger)
            .expect("领域计划必须合法");
    let encoded = encode(command, &plan, previous, QUOTA_LIMIT, at(10, 0)).expect("账本必须可编码");
    (plan, encoded.payload, encoded.proxy)
}

fn tampered(payload: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(payload).expect("账本必须是 JSON");
    edit(&mut value);
    serde_json::to_vec(&value).expect("篡改后的账本必须可编码")
}

fn flip_first_byte(value: &mut serde_json::Value) {
    let byte = value[0].as_u64().expect("UUID 字节必须是数字");
    value[0] = serde_json::Value::from(byte ^ 1);
}

fn assert_corrupt(result: Result<CompetitionCommandOutcome, ApplicationError>) {
    match result {
        Err(ApplicationError::CompetitionTransaction(CompetitionTransactionError::Failed(
            message,
        ))) => assert_eq!(message, "旧版竞价命令账本结果损坏"),
        other => panic!("篡改的账本必须失败关闭: {other:?}"),
    }
}

fn assert_same_bid_response(original: &CompetitionPlan, replayed: &CompetitionCommandOutcome) {
    assert_eq!(replayed.plan.public_bids, original.public_bids);
    assert_eq!(replayed.plan.final_lot.current_price, original.final_lot.current_price);
    assert_eq!(replayed.plan.final_lot.leader_id, original.final_lot.leader_id);
    assert_eq!(replayed.plan.final_lot.state_version, original.final_lot.state_version);
    let expected_end = CompetitionWireTimestamp::from_utc(original.final_lot.effective_end_at);
    assert_eq!(replayed.effective_end_at, expected_end);
    assert_eq!(replayed.effective_end_at.wire(), expected_end.wire());
}

/// `creation_command_id` 不属于旧结果：重放以当前命令 ID 作为内部占位，不会再次持久化。
fn assert_same_proxy_response(
    original: &CompetitionProxyCommandOutcome,
    replayed: &CompetitionProxyCommandOutcome,
    replay_command_id: CommandId,
) {
    let expected =
        ProxyAuthorization { creation_command_id: replay_command_id, ..original.authorization };
    assert_eq!(replayed.authorization, expected);
    assert_eq!(replayed.quota_used, original.quota_used);
    assert_eq!(replayed.quota_limit, original.quota_limit);
    assert_eq!(replayed.terminal_reason, original.terminal_reason);
    assert_eq!(format!("{:?}", replayed.transitions), format!("{:?}", original.transitions));
}

#[test]
fn place_bid_with_fast_proxy_response_replays_identically() {
    let lot_id = LotId::new();
    let (proxy_user, bidder) = (UserId::new(), UserId::new());
    let snapshot = snapshot(
        lot_state(lot_id, 100_000, proxy_user),
        Some(fast_proxy(lot_id, proxy_user, 150_000)),
    );
    let command = CompetitionCommand::PlaceBid(PlaceBidCommand {
        command_id: CommandId::new(),
        lot_id,
        bidder_id: bidder,
        amount: amount(120_000),
        request_hash: RequestHash::new([1; 32]),
        request_id: None,
    });

    let (plan, payload, proxy) = encode_new_command(&command, &snapshot, None, &[]);
    assert_eq!(plan.public_bids.len(), 2, "快速代理必须立即反击");
    assert!(proxy.is_none());

    let replayed = replay(&command, &payload).expect("同一命令必须可重放");
    assert_same_bid_response(&plan, &replayed);
    assert!(replayed.proxy.is_none());

    assert_corrupt(replay(
        &command,
        &tampered(&payload, |value| value["facts"][0]["amount"] = 121_000.into()),
    ));
    assert_corrupt(replay(
        &command,
        &tampered(&payload, |value| flip_first_byte(&mut value["facts"][1]["bid_id"])),
    ));
    assert_corrupt(replay(&command, &tampered(&payload, |value| value["amount"] = 127_000.into())));
}

#[test]
fn create_proxy_results_replay_identically() {
    let lot_id = LotId::new();
    let (leader, bidder, incumbent) = (UserId::new(), UserId::new(), UserId::new());
    let first_use =
        ProxyUsageContext { usage_number: 1, previous_maximum: None, previous_status: None };
    let cases = [
        // 无竞争的 Fast/Slow 新代理按最低价出价。
        (snapshot(lot_state(lot_id, 100_000, leader), None), ProxyMode::Fast, 150_000),
        (snapshot(lot_state(lot_id, 100_000, leader), None), ProxyMode::Slow, 150_000),
        // 低于既有代理上限的新代理被排在下方，账本记录的是既有代理的反击出价。
        (
            snapshot(
                lot_state(lot_id, 100_000, incumbent),
                Some(fast_proxy(lot_id, incumbent, 150_000)),
            ),
            ProxyMode::Fast,
            120_000,
        ),
    ];
    for (snapshot, mode, maximum) in cases {
        let command_id = CommandId::new();
        let command = CompetitionCommand::CreateProxy(CreateProxyCommand {
            command_id,
            lot_id,
            bidder_id: bidder,
            maximum_amount: amount(maximum),
            mode,
            request_hash: RequestHash::new([2; 32]),
            request_id: None,
        });

        let (plan, payload, proxy) = encode_new_command(&command, &snapshot, Some(first_use), &[]);
        let original = proxy.expect("创建代理必须冻结代理响应");

        let replayed = replay(&command, &payload).expect("同一命令必须可重放");
        assert_same_bid_response(&plan, &replayed);
        let replayed_proxy = replayed.proxy.as_ref().expect("重放必须返回代理响应");
        assert_same_proxy_response(&original, replayed_proxy, command_id);

        let other_mode = if mode == ProxyMode::Fast { "slow" } else { "fast" };
        assert_corrupt(replay(
            &command,
            &tampered(&payload, |value| value["mode"] = other_mode.into()),
        ));
        assert_corrupt(replay(
            &command,
            &tampered(&payload, |value| value["maximum_amount"] = (maximum + 1_000).into()),
        ));
        assert_corrupt(replay(
            &command,
            &tampered(&payload, |value| flip_first_byte(&mut value["authorization_id"])),
        ));
    }
}

#[test]
fn lower_proxy_result_replays_identically() {
    let lot_id = LotId::new();
    let holder = UserId::new();
    let active = fast_proxy(lot_id, holder, 150_000);
    let snapshot = snapshot(lot_state(lot_id, 120_000, holder), Some(active));
    let created = CompetitionProxyTransitionRecord {
        transition_number: 1,
        kind: "created".to_owned(),
        old_maximum_amount: None,
        new_maximum_amount: 150_000,
        old_mode: None,
        new_mode: "fast".to_owned(),
        old_status: None,
        new_status: "active".to_owned(),
        reason: None,
        resulting_state_version: 1,
        created_at: at(9, 0),
    };
    let command_id = CommandId::new();
    let command = CompetitionCommand::LowerProxy(LowerProxyCommand {
        command_id,
        lot_id,
        bidder_id: holder,
        authorization_id: active.id,
        new_maximum: amount(130_000),
        request_hash: RequestHash::new([3; 32]),
        request_id: None,
    });

    let (plan, payload, proxy) =
        encode_new_command(&command, &snapshot, None, std::slice::from_ref(&created));
    assert!(plan.public_bids.is_empty());
    let original = proxy.expect("降低代理必须冻结代理响应");
    assert_eq!(original.transitions.len(), 2, "响应必须携带完整转换历史");

    let replayed = replay(&command, &payload).expect("同一命令必须可重放");
    assert!(replayed.plan.public_bids.is_empty());
    let replayed_proxy = replayed.proxy.as_ref().expect("重放必须返回代理响应");
    assert_same_proxy_response(&original, replayed_proxy, command_id);

    assert_corrupt(replay(
        &command,
        &tampered(&payload, |value| value["maximum_amount"] = 140_000.into()),
    ));
    assert_corrupt(replay(
        &command,
        &tampered(&payload, |value| flip_first_byte(&mut value["authorization_id"])),
    ));
}

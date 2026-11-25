//! v2 回执与 `consignment_commands.result_body` 的稳定映射。
//!
//! 4A 的回执类型是本模块的唯一来源；HTTP 只消费解码后的 4A 值，存储只比较与落库
//! 应用层算好的摘要。字段名与 `000134` 的 `c4_valid_v2_receipt` 逐字一致，内部字段
//! （如 `held_reason`）允许留在账本但不进入公开 DTO。

mod control;
mod envelope;
mod loss;

use auction_application::consignment::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ListingStateChange, ReceiptDisposition,
    ReservationReceipt, ReservationRetired, SeasonCloseReceipt, SeasonCloseTarget,
    SeasonNoticeReceipt,
};
use auction_application::core::lower_hex;
use auction_domain::CommandId;
use auction_time::Timestamp;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::errors::WriteError;
use super::ledger::StoredReceipt;

/// 一种 4A v2 成功回执的账本映射。
pub(crate) trait DepositReceipt: Sized {
    /// 固定操作。
    fn operation(&self) -> DepositOperation;
    /// 首次状态码。
    fn status(&self) -> i16;
    /// 成功处置。
    fn disposition(&self) -> ReceiptDisposition;
    /// 命令 UUID。
    fn command_id(&self) -> CommandId;
    /// 完整账本体。
    fn body(&self) -> Value;
    /// 回执中逐件 L；无 L 的回执返回空列表。
    fn listing_changes(&self) -> Vec<ListingStateChange> {
        Vec::new()
    }
    /// 从首次账本行解回 4A 类型。
    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError>;
}

fn corrupt() -> WriteError {
    WriteError::invariant("consignment_v2_receipt")
}

fn corrupt_from<T>(_: T) -> WriteError {
    corrupt()
}

fn common(
    operation: DepositOperation,
    disposition: ReceiptDisposition,
    command_id: CommandId,
) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("schema_version".into(), json!(DEPOSIT_SCHEMA_VERSION));
    body.insert("operation".into(), json!(operation.as_str()));
    body.insert("disposition".into(), json!(disposition.as_str()));
    body.insert("command_id".into(), json!(command_id.as_uuid().to_string()));
    body
}

fn finish_body(mut fields: Map<String, Value>) -> Value {
    Value::Object(std::mem::take(&mut fields))
}

fn listing_change(change: &ListingStateChange) -> Value {
    json!({
        "listing_id": change.listing_id.to_string(),
        "listing_version": change.listing_version,
        "state": change.state.as_str(),
    })
}

fn listing_changes(changes: &[ListingStateChange]) -> Value {
    Value::Array(changes.iter().map(listing_change).collect())
}

fn digest(bytes: [u8; 32]) -> Value {
    json!(lower_hex(&bytes))
}

fn parse_digest(value: Value) -> Result<[u8; 32], WriteError> {
    let text = value.as_str().ok_or_else(corrupt)?;
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(corrupt());
    }
    let mut bytes = [0_u8; 32];
    for (index, chunk) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let chunk = std::str::from_utf8(chunk).map_err(corrupt_from)?;
        bytes[index] = u8::from_str_radix(chunk, 16).map_err(corrupt_from)?;
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct CommonBody {
    schema_version: u16,
    operation: String,
    disposition: String,
    command_id: Uuid,
}

fn common_body(
    stored: &StoredReceipt,
    operation: DepositOperation,
) -> Result<CommonBody, WriteError> {
    let common: CommonBody = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
    if common.schema_version != DEPOSIT_SCHEMA_VERSION
        || common.operation != operation.as_str()
        || common.command_id.is_nil()
    {
        return Err(corrupt());
    }
    Ok(common)
}

fn require_status<T: DepositReceipt>(
    stored: &StoredReceipt,
    common: &CommonBody,
    receipt: T,
) -> Result<T, WriteError> {
    if stored.status != receipt.status()
        || common.disposition != receipt.disposition().as_str()
        || common.command_id != receipt.command_id().as_uuid()
    {
        return Err(corrupt());
    }
    Ok(receipt)
}

#[derive(Deserialize)]
struct ListingBody {
    listing_id: Uuid,
    listing_version: u64,
    state: String,
}

fn listing_body(value: Value) -> Result<ListingStateChange, WriteError> {
    let body: ListingBody = serde_json::from_value(value).map_err(corrupt_from)?;
    if body.listing_id.is_nil() || body.listing_version == 0 {
        return Err(corrupt());
    }
    Ok(ListingStateChange {
        listing_id: body.listing_id,
        listing_version: body.listing_version,
        state: body.state.parse().map_err(corrupt_from)?,
    })
}

fn decode_listing_changes(value: Option<Value>) -> Result<Vec<ListingStateChange>, WriteError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.into_iter().map(listing_body).collect(),
        Some(_) => Err(corrupt()),
    }
}

impl DepositReceipt for ReservationReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::IssueReservation
    }

    fn status(&self) -> i16 {
        201
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::ReservationIssued
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("listing_id".into(), json!(self.listing_id.to_string()));
        body.insert("listing_version".into(), json!(self.listing_version.value()));
        body.insert("state".into(), json!(self.state.as_str()));
        body.insert("reservation_id".into(), json!(self.reservation_id.to_string()));
        body.insert("expires_at".into(), json!(self.expires_at));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            listing_id: Uuid,
            listing_version: u64,
            state: String,
            reservation_id: Uuid,
            expires_at: Timestamp,
        }
        let common = common_body(stored, DepositOperation::IssueReservation)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        let listing_version =
            auction_domain::consignment::ListingVersion::new(body.listing_version)
                .map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self {
                schema_version: DEPOSIT_SCHEMA_VERSION,
                operation: DepositOperation::IssueReservation,
                disposition: ReceiptDisposition::ReservationIssued,
                command_id: CommandId::from_uuid(common.command_id),
                listing_id: body.listing_id,
                listing_version,
                state: body.state.parse().map_err(corrupt_from)?,
                reservation_id: body.reservation_id,
                expires_at: body.expires_at,
            },
        )
    }
}

impl DepositReceipt for ReservationRetired {
    fn operation(&self) -> DepositOperation {
        DepositOperation::ExpireReservation
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::ReservationRetired
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("reservation_id".into(), json!(self.reservation_id.to_string()));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            reservation_id: Uuid,
        }
        let common = common_body(stored, DepositOperation::ExpireReservation)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::new(CommandId::from_uuid(common.command_id), body.reservation_id),
        )
    }
}

impl DepositReceipt for SeasonNoticeReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::SeasonNotice
    }

    fn status(&self) -> i16 {
        202
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::NoticeQueued
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("season_id".into(), json!(self.season_id));
        body.insert("position_id".into(), json!(self.position_id.to_string()));
        body.insert("seller_user_id".into(), json!(self.seller_user_id.to_string()));
        body.insert("work_id".into(), json!(self.work_id.to_string()));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            season_id: String,
            position_id: Uuid,
            seller_user_id: Uuid,
            work_id: Uuid,
        }
        let common = common_body(stored, DepositOperation::SeasonNotice)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::queued(
                CommandId::from_uuid(common.command_id),
                body.season_id,
                body.position_id,
                auction_domain::UserId::from_uuid(body.seller_user_id),
                body.work_id,
            ),
        )
    }
}

fn season_target(target: SeasonCloseTarget) -> Value {
    match target {
        SeasonCloseTarget::World(world) => json!({
            "kind": "world",
            "world": auction_domain::consignment::settlement_world_code(world),
        }),
        SeasonCloseTarget::Listing(listing_id) => {
            json!({"kind": "listing", "id": listing_id.to_string()})
        }
        SeasonCloseTarget::Position(position_id) => {
            json!({"kind": "position", "id": position_id.to_string()})
        }
    }
}

#[derive(Deserialize)]
struct SeasonTargetBody {
    kind: String,
    id: Option<Uuid>,
    world: Option<String>,
}

fn decode_season_target(value: Value) -> Result<SeasonCloseTarget, WriteError> {
    let body: SeasonTargetBody = serde_json::from_value(value).map_err(corrupt_from)?;
    match body.kind.as_str() {
        "world" => {
            let world = body
                .world
                .as_deref()
                .and_then(auction_domain::consignment::parse_settlement_world)
                .ok_or_else(corrupt)?;
            Ok(SeasonCloseTarget::World(world))
        }
        "listing" => Ok(SeasonCloseTarget::Listing(body.id.ok_or_else(corrupt)?)),
        "position" => Ok(SeasonCloseTarget::Position(body.id.ok_or_else(corrupt)?)),
        _ => Err(corrupt()),
    }
}

impl DepositReceipt for SeasonCloseReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::SeasonClose
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::SeasonClosed
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("season_id".into(), json!(self.season_id));
        body.insert("target".into(), season_target(self.target));
        body.insert(
            "closed_listing_ids".into(),
            Value::Array(self.closed_listing_ids.iter().map(|id| json!(id.to_string())).collect()),
        );
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            season_id: String,
            target: Value,
            closed_listing_ids: Vec<Uuid>,
        }
        let common = common_body(stored, DepositOperation::SeasonClose)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::closed(
                CommandId::from_uuid(common.command_id),
                body.season_id,
                decode_season_target(body.target)?,
                body.closed_listing_ids,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use auction_application::consignment::{
        CustodyListingChange, EnvelopeDisposition, EnvelopeReceipt, ListingStateChange,
        ObservationReceipt, OperationsHoldDisposition, OperationsHoldReceipt, ProofDisposition,
        ProofReceipt, QuarantineReference, RecheckReceipt, ReleaseReceipt, SeasonCloseTarget,
        StartReceipt, WithdrawDepositReceipt, WithdrawDisposition,
    };
    use auction_domain::consignment::deposit::{PositionState, ReleaseKind};
    use auction_domain::consignment::{HeldOffMarketReason, ListingVersion};
    use auction_domain::{CommandId, SettlementWorld, UserId};

    use super::DepositReceipt;
    use super::ReservationReceipt;
    use super::ReservationRetired;
    use super::SeasonCloseReceipt;
    use super::SeasonNoticeReceipt;
    use super::StoredReceipt;
    use super::Timestamp;
    use super::Uuid;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn command(n: u128) -> CommandId {
        CommandId::from_uuid(id(n))
    }

    fn stored<T: DepositReceipt>(receipt: &T) -> StoredReceipt {
        StoredReceipt { status: receipt.status(), code: "ok".into(), body: receipt.body() }
    }

    fn assert_roundtrip<T>(receipt: T)
    where
        T: DepositReceipt + std::fmt::Debug + PartialEq,
    {
        let stored = stored(&receipt);
        assert_eq!(T::decode(&stored).expect("解码回执"), receipt);
    }

    #[test]
    fn reservation_and_control_receipts_roundtrip() {
        assert_roundtrip(ReservationReceipt::issued(
            command(1),
            id(2),
            ListingVersion::new(3).expect("版本"),
            auction_domain::consignment::ListingState::AwaitingDeposit,
            id(4),
            Timestamp::parse_from_rfc3339("2026-01-01T12:00:00Z").expect("时刻"),
        ));
        assert_roundtrip(ReservationRetired::new(command(5), id(6)));
        assert_roundtrip(WithdrawDepositReceipt::new(
            command(7),
            WithdrawDisposition::WithdrawalRequested,
            id(8),
            ListingVersion::new(3).expect("版本"),
            auction_domain::consignment::ListingState::DepositAccepted,
            Some(id(9)),
        ));
        assert_roundtrip(WithdrawDepositReceipt::withdrawal_accepted(
            command(70),
            id(71),
            ListingVersion::new(5).expect("版本"),
            id(72),
            id(73),
        ));
        assert_roundtrip(WithdrawDepositReceipt::new(
            command(10),
            WithdrawDisposition::Closed,
            id(11),
            ListingVersion::new(4).expect("版本"),
            auction_domain::consignment::ListingState::Closed,
            None,
        ));
        assert_roundtrip(
            OperationsHoldReceipt::new(
                command(12),
                OperationsHoldDisposition::HoldSet,
                id(13),
                ListingVersion::new(3).expect("版本"),
                auction_domain::consignment::ListingState::HeldOffMarket,
                Some(id(14)),
                Some(HeldOffMarketReason::OperationsHold),
            )
            .expect("止售设置回执"),
        );
        assert_roundtrip(
            OperationsHoldReceipt::new(
                command(15),
                OperationsHoldDisposition::HoldReleased,
                id(16),
                ListingVersion::new(4).expect("版本"),
                auction_domain::consignment::ListingState::HeldOffMarket,
                Some(id(17)),
                Some(HeldOffMarketReason::Review),
            )
            .expect("止售解除回执"),
        );
        assert_roundtrip(
            OperationsHoldReceipt::new(
                command(18),
                OperationsHoldDisposition::Closed,
                id(19),
                ListingVersion::new(5).expect("版本"),
                auction_domain::consignment::ListingState::Closed,
                None,
                None,
            )
            .expect("未代管下架回执"),
        );
    }

    /// 反向解码拒绝与处置不符的 hold/状态组合，避免坏回执被当成可重放成功。
    #[test]
    fn operations_hold_decode_rejects_illegal_hold_shapes() {
        use serde_json::json;

        fn stored(
            operation: &str,
            disposition: &str,
            state: &str,
            hold_id: Option<Uuid>,
            held_reason: Option<&str>,
        ) -> StoredReceipt {
            let mut body = json!({
                "schema_version": 2,
                "operation": operation,
                "disposition": disposition,
                "command_id": id(70).to_string(),
                "listing_id": id(71).to_string(),
                "listing_version": 3,
                "state": state,
                "held_reason": held_reason,
            });
            if let Some(hold_id) = hold_id {
                body["hold_id"] = json!(hold_id.to_string());
            }
            StoredReceipt { status: 200, code: "ok".into(), body }
        }

        let illegal = [
            stored("admin_delist", "closed", "closed", Some(id(72)), None),
            stored("admin_delist", "closed", "closed", None, Some("operations_hold")),
            stored("admin_delist", "hold_set", "held_off_market", None, Some("operations_hold")),
            stored("release_hold", "hold_released", "held_off_market", None, Some("review")),
            stored("admin_delist", "hold_set", "closed", Some(id(73)), Some("operations_hold")),
            stored("release_hold", "hold_set", "held_off_market", Some(id(74)), Some("review")),
        ];
        for (index, stored) in illegal.into_iter().enumerate() {
            assert!(
                OperationsHoldReceipt::decode(&stored).is_err(),
                "非法 hold 组合 {index} 必须解码失败"
            );
        }
    }

    #[test]
    fn envelope_observation_and_proof_receipts_roundtrip() {
        let mut envelope = EnvelopeReceipt::new(command(20), EnvelopeDisposition::Accepted, id(21));
        envelope.listing_changes = vec![ListingStateChange {
            listing_id: id(22),
            listing_version: 3,
            state: auction_domain::consignment::ListingState::DepositAccepted,
        }];
        assert_roundtrip(envelope);
        let mut quarantine =
            EnvelopeReceipt::new(command(23), EnvelopeDisposition::Quarantined, id(24));
        quarantine.quarantine = Some(QuarantineReference { quarantine_id: id(25), version: 1 });
        assert_roundtrip(quarantine);
        assert_roundtrip(ObservationReceipt::appended(command(26), id(27), 2, id(28), [0x29; 32]));
        assert_roundtrip(RecheckReceipt::queued(command(30), id(31), id(32)));
        let mut proof =
            ProofReceipt::new(command(33), ProofDisposition::TakeProven, id(34), id(35));
        proof.listing_changes = vec![ListingStateChange {
            listing_id: id(36),
            listing_version: 3,
            state: auction_domain::consignment::ListingState::HeldOffMarket,
        }];
        proof.custody_changes = vec![CustodyListingChange {
            listing_id: id(36),
            listing_version: 3,
            state: auction_domain::consignment::ListingState::HeldOffMarket,
            held_reason: Some(HeldOffMarketReason::OperationsHold),
            position_id: id(37),
            position_state: PositionState::InCustody,
        }];
        assert_roundtrip(proof);
    }

    #[test]
    fn release_start_and_season_receipts_roundtrip() {
        let mut release =
            ReleaseReceipt::released(command(40), id(41), id(42), id(43), ReleaseKind::Accept);
        release.listing_changes = vec![ListingStateChange {
            listing_id: id(44),
            listing_version: 5,
            state: auction_domain::consignment::ListingState::DepositAccepted,
        }];
        assert_roundtrip(release);
        let mut case = ReleaseReceipt::released(
            command(45),
            id(46),
            id(47),
            id(48),
            ReleaseKind::AuctionReturnCase,
        );
        case.obligation_id = None;
        case.return_mail_id = Some(id(49));
        case.return_evidence_id = Some(id(50));
        assert_roundtrip(case);
        assert_roundtrip(StartReceipt::started(command(51), id(52), id(53), id(54), 7));
        assert_roundtrip(SeasonNoticeReceipt::queued(
            command(55),
            "2026-11".into(),
            id(56),
            UserId::from_uuid(id(57)),
            id(58),
        ));
        assert_roundtrip(SeasonCloseReceipt::closed(
            command(59),
            "2026-11".into(),
            SeasonCloseTarget::World(SettlementWorld::Main),
            vec![id(60)],
        ));
    }
}

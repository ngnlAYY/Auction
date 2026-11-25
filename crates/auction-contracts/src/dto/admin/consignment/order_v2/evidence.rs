//! Admin 订单证据分页：各轮、尝试、退件接管、观察、现金用途与阻断。

use auction_time::Timestamp;
use serde::Deserialize;
use uuid::Uuid;

use crate::DeliveryStageV2;
use crate::LegStageV2;
use crate::ListingPage;
use crate::OrderResolveActionV2;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::wire_v2::sha256_hex;
use crate::dto::public::consignment::wire_v2::string_enum;

/// 证据分页，页壳与挂单分页一致。
pub type OrderEvidencePageV2 = ListingPage<OrderEvidenceItemV2>;

/// 现金腿金额上限：标价上限十亿再加载体本金 1。
const CASH_AMOUNT_MAX: i64 = 1_000_000_001;

string_enum! {
    /// 派发轮所属的腿。
    pub enum EvidenceLegV2 {
        /// 原件。
        Original => "original",
        /// 载体。
        Carrier => "carrier",
        /// 卖款。
        Payout => "payout",
        /// 退款。
        Refund => "refund",
    }
}

string_enum! {
    /// 退件接管的进度。
    pub enum ReturnTakeStateV2 {
        /// 已建接管，尚未起写。
        Pending => "pending",
        /// 已起写，等待精确 Take 证明。
        WriteStarted => "write_started",
        /// 已证精确 Take，尚未消费。
        Taken => "taken",
        /// 已消费。
        Consumed => "consumed",
        /// 证据不足，持续核对。
        Reconcile => "reconcile",
    }
}

string_enum! {
    /// 远端观察的类别。
    pub enum ObservationCategoryV2 {
        /// 已发邮件。
        Sent => "sent",
        /// Take 明细。
        Take => "take",
        /// 系统退件。
        Rts => "rts",
        /// 付款。
        Payment => "payment",
        /// 确领。
        Claim => "claim",
    }
}

string_enum! {
    /// 观察对应的结论。观察本身不自动成为有效结论。
    pub enum ObservationConclusionV2 {
        /// 尚未结论。
        Pending => "pending",
        /// 已采信。
        Accepted => "accepted",
        /// 已拒绝。
        Rejected => "rejected",
        /// 与既有结论冲突。
        Conflict => "conflict",
    }
}

string_enum! {
    /// 现金用途。
    pub enum CashPurposeV2 {
        /// 卖款。
        Seller => "seller",
        /// 手续费。
        Fee => "fee",
        /// 载体本金。
        Carrier => "carrier",
        /// 买家退款。
        Refund => "refund",
        /// 保留的受益用途。
        Retained => "retained",
    }
}

string_enum! {
    /// 阻断类别。
    pub enum BlockerKindV2 {
        /// 本 UID 现金 opening 阻断。
        OpeningUidBlock => "opening_uid_block",
        /// 相反付款仍然开放。
        OppositePaymentOpen => "opposite_payment_open",
        /// 资金边界不明。
        FundingBoundary => "funding_boundary",
        /// 存在结果未知的外写。
        UnknownExternalWrite => "unknown_external_write",
    }
}

string_enum! {
    /// Sale 封闭事实的种类。
    pub enum SaleClosureKindV2 {
        /// 买家从未付款、从未确领，购买被取消。
        CancelledUnpaid => "cancelled_unpaid",
        /// 已付款但退款已由 6B 单写者转换。
        RefundClosed => "refund_closed",
    }
}

/// 键必须出现；值可以为 null，非空值是正的安全整数。
fn required_option_positive<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Positive(#[serde(deserialize_with = "positive_version")] i64);

    Option::<Positive>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

/// 序列化前复验可空的正安全整数。
fn serialize_option_positive<S: serde::Serializer>(
    value: &Option<i64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serialize_positive_version(value, serializer),
        None => serializer.serialize_none(),
    }
}

json_object! {
    (tag = "kind", rename_all = "snake_case")
    /// 订单证据条目。只含 ID、阶段、金额、摘要与时刻，不含预约码、装备 key 与原信正文。
    #[derive(Debug, Clone, PartialEq)]
    pub enum OrderEvidenceItemV2 {
        /// 派发轮。
        Round {
            #[serde(deserialize_with = "uuid")]
            round_id: Uuid,
            #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
            round_number: i64,
            leg: EvidenceLegV2,
            delivery_stage: DeliveryStageV2,
            #[serde(deserialize_with = "timestamp")]
            created_at: Timestamp,
        },
        /// 派发尝试及其 fence。
        Attempt {
            #[serde(deserialize_with = "uuid")]
            attempt_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            round_id: Uuid,
            #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
            fence: i64,
            write_started: bool,
            delivery_stage: DeliveryStageV2,
        },
        /// 退件接管。
        ReturnTake {
            #[serde(deserialize_with = "uuid")]
            intake_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            work_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            original_round_id: Uuid,
            #[serde(deserialize_with = "required_option_positive", serialize_with = "serialize_option_positive")]
            fence: Option<i64>,
            state: ReturnTakeStateV2,
        },
        /// 远端观察及其结论。
        Observation {
            #[serde(deserialize_with = "uuid")]
            observation_id: Uuid,
            category: ObservationCategoryV2,
            conclusion: ObservationConclusionV2,
            #[serde(deserialize_with = "sha256_hex")]
            digest: String,
            #[serde(deserialize_with = "timestamp")]
            observed_at: Timestamp,
        },
        /// 现金用途。
        CashUse {
            purpose: CashPurposeV2,
            state: LegStageV2,
            amount: i64,
        },
        /// 阻断。
        Blocker {
            blocker: BlockerKindV2,
            #[serde(deserialize_with = "timestamp")]
            since: Timestamp,
            resolved: bool,
        },
        /// Sale 封闭事实（恢复段）。
        Closure {
            closure_kind: SaleClosureKindV2,
            #[serde(deserialize_with = "timestamp")]
            closed_at: Timestamp,
        },
        /// Admin 处置记录（恢复段）。
        Resolution {
            #[serde(deserialize_with = "uuid")]
            resolution_id: Uuid,
            action: OrderResolveActionV2,
            #[serde(deserialize_with = "uuid")]
            evidence_id: Uuid,
            #[serde(deserialize_with = "timestamp")]
            resolved_at: Timestamp,
        },
        /// Admin 排入的复核工作（恢复段）。
        Recheck {
            #[serde(deserialize_with = "uuid")]
            work_id: Uuid,
            #[serde(deserialize_with = "timestamp")]
            requested_at: Timestamp,
        },
    } => validate
}

impl OrderEvidenceItemV2 {
    fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Attempt { write_started, delivery_stage, .. } => {
                // 起写事实先于 POST：未起写只能是 prepared，起写后不得回到未开始。
                let started_stage = !matches!(
                    delivery_stage,
                    DeliveryStageV2::NotStarted | DeliveryStageV2::Prepared
                );
                if *write_started != started_stage {
                    return Err("尝试的起写标记与交割阶段不一致");
                }
            }
            Self::ReturnTake { fence, state, .. } => {
                if fence.is_none() != (*state == ReturnTakeStateV2::Pending) {
                    return Err("退件接管只有未起写时才没有 fence");
                }
            }
            Self::CashUse { state, amount, .. } => {
                if *state == LegStageV2::NotApplicable || !(0..=CASH_AMOUNT_MAX).contains(amount) {
                    return Err("现金用途的阶段或金额超出范围");
                }
            }
            Self::Round { .. }
            | Self::Observation { .. }
            | Self::Blocker { .. }
            | Self::Closure { .. }
            | Self::Resolution { .. }
            | Self::Recheck { .. } => {}
        }
        Ok(())
    }
}

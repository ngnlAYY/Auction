//! Admin 隔离详情、观察投影、重查/解除请求与回执，以及运营止售回执。

mod validation;

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::ConsignmentDispositionV2;
use crate::ConsignmentOperationV2;
use crate::ListingStampV2;
use crate::ListingStateV2;
use crate::ListingVersion;
use crate::ListingWorld;
use crate::deserialize_required_option;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::external_mid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::required_option_uuid;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::wire_v2::sha256_hex;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

const MAX_OBSERVATIONS: usize = 200;
const MAX_ACCEPT_ITEMS: usize = 20;

/// 隔离版本从 1 开始，必须能由浏览器精确表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantineVersion(i64);

impl QuarantineVersion {
    /// 从已校验的安全整数构造；HTTP 读投影只在正数范围内解封。
    pub const fn new(value: i64) -> Option<Self> {
        if value >= 1 && value <= crate::dto::public::consignment::wire_v2::SAFE_INTEGER_MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// 返回隔离版本整数。
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for QuarantineVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        positive_version(deserializer).map(Self)
    }
}

impl Serialize for QuarantineVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_positive_version(&self.0, serializer)
    }
}

string_enum! {
    /// 隔离投影的持久态。
    pub enum QuarantineState {
        /// 仍由共享隔离持有原信终局槽。
        Active => "active",
        /// 已解除并交给唯一 owner。
        Released => "released",
    }
}

string_enum! {
    /// 隔离责任码；逐字对齐第 4 期领域枚举 `QuarantineReason` 的 12 个码。
    pub enum QuarantineReason {
        /// 邮件包含重复原文 EID，解析失败。
        DuplicateRawEid => "duplicate_raw_eid",
        /// `42` 与 `00042` 等同世界别名无法唯一匹配。
        CanonicalAliasAmbiguous => "canonical_alias_ambiguous",
        /// 附件布局或种类未知。
        UnknownAttachment => "unknown_attachment",
        /// 列表与详情清单冲突。
        ListDetailConflict => "list_detail_conflict",
        /// 资产数量无法确定。
        AssetCountUnknown => "asset_count_unknown",
        /// 同名显示身份换了认证 UID。
        SameNameChangedUid => "same_name_changed_uid",
        /// 发件人或认证身份无法证明。
        IdentityUnproven => "identity_unproven",
        /// 金额/COD 形状无法证明。
        AmountUnproven => "amount_unproven",
        /// 无法证明可安全原样退回。
        ReturnUnproven => "return_unproven",
        /// 取得能力无法证明（第 4 期取得验证器未签收时整封隔离）。
        TakeUnproven => "take_unproven",
        /// 邮件形状不能安全原样退回或会扣款。
        UnsafeReturnShape => "unsafe_return_shape",
        /// 疑似系统 RTS，但既有拍卖路由无法归属。
        SuspectedSystemRts => "suspected_system_rts",
    }
}

string_enum! {
    /// Admin 在隔离详情上可执行的后续动作。
    pub enum QuarantineAction {
        /// 排入原信/原会话的只读重查。
        Recheck => "recheck",
        /// 引用一条具体新观察解除隔离。
        Release => "release",
    }
}

string_enum! {
    /// 脱敏清单中的附件种类。
    pub enum QuarantineObservationItemKind {
        /// 可识别装备。
        Equipment => "equipment",
        /// Credits 金额。
        Credits => "credits",
        /// COD 金额。
        Cod => "cod",
        /// 尚未识别的附件。
        Unknown => "unknown",
    }
}

string_enum! {
    /// 隔离解除的六种最终决定。
    pub enum QuarantineDecisionKind {
        /// 整封接受。
        Accept => "accept",
        /// 原样退回。
        Return => "return",
        /// 空信归档。
        ArchiveEmpty => "archive_empty",
        /// 寄售原轮退件接管；第 4 期只预留类型。
        AcquireReturn => "acquire_return",
        /// 交给原拍卖退件案件。
        AuctionReturnCase => "auction_return_case",
        /// 交给原拍卖恢复记录。
        AuctionRecoveryReturn => "auction_recovery_return",
    }
}

json_object! {
    ()
    /// 脱敏清单的一项；不回显原 token、正文、装备 key 或受保护引用。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct QuarantineObservationItem {
        pub ordinal: i64,
        pub kind: QuarantineObservationItemKind,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub external_equipment_id: Option<String>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub amount: Option<i64>,
    } => validate
}

json_object! {
    ()
    /// 一条追加观察的安全投影；原文只在受控证据中保存。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct QuarantineObservation {
        #[serde(deserialize_with = "uuid")]
        pub observation_id: Uuid,
        #[serde(deserialize_with = "timestamp")]
        pub observed_at: Timestamp,
        #[serde(deserialize_with = "text::<64, _>")]
        pub parser_version: String,
        #[serde(deserialize_with = "sha256_hex")]
        pub digest: String,
        pub attachment_count: i64,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub credits: Option<i64>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub cod: Option<i64>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub returnable: Option<bool>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub takeable: Option<bool>,
        pub items: Vec<QuarantineObservationItem>,
    } => validate
}

json_object! {
    (tag = "decision_kind", rename_all = "snake_case")
    /// 隔离解除后的最终决定与 owner 引用。
    #[derive(Debug, Clone, PartialEq)]
    pub enum QuarantineReleaseOutcome {
        /// 普通寄件整封接受，逐件给出 L。
        Accept { listing_changes: Vec<ListingStampV2> },
        /// 原样退回。
        Return {},
        /// 空信归档。
        ArchiveEmpty {},
        /// 寄售原轮退件接管；第 4 期正式适配器不能产生该成功回执。
        AcquireReturn {
            #[serde(deserialize_with = "uuid")]
            return_intake_id: Uuid,
            original_round: i64,
            #[serde(deserialize_with = "uuid")]
            take_work_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            take_delivery_id: Uuid,
        },
        /// 交给原拍卖退件案件；E2 允许 obligation_id 为空。
        AuctionReturnCase {
            #[serde(deserialize_with = "uuid")]
            return_mail_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            return_evidence_id: Uuid,
            #[serde(deserialize_with = "required_option_uuid")]
            obligation_id: Option<Uuid>,
        },
        /// 交给原拍卖恢复记录。
        AuctionRecoveryReturn {
            #[serde(deserialize_with = "uuid")]
            obligation_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            return_mail_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            recovery_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            recovery_handoff_id: Uuid,
        },
    } => validate
}

json_object! {
    ()
    /// 已解除隔离的最终决定记录。
    #[derive(Debug, Clone, PartialEq)]
    pub struct QuarantineReleaseRecord {
        #[serde(deserialize_with = "uuid")]
        pub release_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub decision_id: Uuid,
        #[serde(deserialize_with = "timestamp")]
        pub released_at: Timestamp,
        pub outcome: QuarantineReleaseOutcome,
    } => validate
}

json_object! {
    ()
    /// Admin 隔离详情。只返回安全投影，不含原证密文、正文、预约码或装备 key。
    #[derive(Debug, Clone, PartialEq)]
    pub struct QuarantineDetail {
        #[serde(deserialize_with = "uuid")]
        pub quarantine_id: Uuid,
        pub version: QuarantineVersion,
        pub state: QuarantineState,
        pub world: ListingWorld,
        #[serde(deserialize_with = "external_mid")]
        pub external_mid: String,
        #[serde(deserialize_with = "uuid")]
        pub first_decision_id: Uuid,
        pub reason: QuarantineReason,
        /// 累计观察总数；`observations` 只含最新至多 200 条。
        pub observation_total: i64,
        pub observations: Vec<QuarantineObservation>,
        pub allowed_actions: Vec<QuarantineAction>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub release: Option<QuarantineReleaseRecord>,
    } => validate
}

json_object! {
    (tag = "decision_kind", rename_all = "snake_case")
    /// 解除隔离的 200 回执；分支字段与最终决定平铺，未列字段一律拒绝。
    #[derive(Debug, Clone, PartialEq)]
    pub enum QuarantineReleaseReceipt {
        /// 普通寄件整封接受。
        Accept {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            disposition: ConsignmentDispositionV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            quarantine_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            release_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            decision_id: Uuid,
            listing_changes: Vec<ListingStampV2>,
        },
        /// 原样退回。
        Return {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            disposition: ConsignmentDispositionV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            quarantine_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            release_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            decision_id: Uuid,
        },
        /// 空信归档。
        ArchiveEmpty {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            disposition: ConsignmentDispositionV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            quarantine_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            release_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            decision_id: Uuid,
        },
        /// 寄售原轮退件接管。
        AcquireReturn {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            disposition: ConsignmentDispositionV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            quarantine_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            release_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            decision_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            return_intake_id: Uuid,
            original_round: i64,
            #[serde(deserialize_with = "uuid")]
            take_work_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            take_delivery_id: Uuid,
        },
        /// 交给原拍卖退件案件。
        AuctionReturnCase {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            disposition: ConsignmentDispositionV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            quarantine_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            release_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            decision_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            return_mail_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            return_evidence_id: Uuid,
            #[serde(deserialize_with = "required_option_uuid")]
            obligation_id: Option<Uuid>,
        },
        /// 交给原拍卖恢复记录。
        AuctionRecoveryReturn {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            disposition: ConsignmentDispositionV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            quarantine_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            release_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            decision_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            obligation_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            return_mail_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            recovery_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            recovery_handoff_id: Uuid,
        },
    } => validate
}

write_object! {
    /// Admin 重查隔离。不变量 1、6：稳定 UUIDv4 与预期版本。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct QuarantineRecheckRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: QuarantineVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
    }
}

json_object! {
    ()
    /// 重查已排入的 202 回执。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct QuarantineRecheckReceipt {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: ConsignmentOperationV2,
        pub disposition: ConsignmentDispositionV2,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub quarantine_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub work_id: Uuid,
    } => validate
}

write_object! {
    /// Admin 解除隔离。不得提供跳过证明的 force 字段。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct QuarantineReleaseRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: QuarantineVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        pub observation_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub proposed_kind: QuarantineDecisionKind => ("serde::Deserialize::deserialize", "必须为六种解除结果之一"),
    }
}

json_object! {
    ()
    /// 运营止售设置或解除的 200 回执；L 与 hold ID 均为必填。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ConsignmentHoldReceipt {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: ConsignmentOperationV2,
        pub disposition: ConsignmentDispositionV2,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub listing_version: ListingVersion,
        pub state: ListingStateV2,
        #[serde(deserialize_with = "uuid")]
        pub hold_id: Uuid,
    } => validate
}

json_object! {
    (tag = "disposition", rename_all = "snake_case")
    /// Admin 下架 v2 的两种合法回执：未代管件关闭，代管件设置独立止售。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ConsignmentDelistReceiptV2 {
        /// 未代管件已按前三期规则关闭。
        Closed {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
        },
        /// 已代管件设置了独立运营止售。
        HoldSet {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
            #[serde(deserialize_with = "uuid")]
            hold_id: Uuid,
        },
    } => validate
}

write_object! {
    /// Admin 解除指定运营止售。不变量 1、6：稳定 UUIDv4、预期版本与原 hold。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ReleaseConsignmentHoldRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        pub hold_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::QuarantineReleaseOutcome;
    use super::ReleaseConsignmentHoldRequest;
    use crate::dto::public::consignment::wire_v2::is_equipment_id;

    #[test]
    fn observation_equipment_id_is_canonical() {
        assert!(is_equipment_id("42"));
        assert!(!is_equipment_id("042"));
    }

    #[test]
    fn digest_helper_rejects_uppercase() {
        use crate::dto::public::consignment::wire_v2::is_sha256_hex;

        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(!is_sha256_hex(&"A".repeat(64)));
    }

    #[test]
    fn recovery_return_validation_rejects_nil_references() {
        let valid = Uuid::from_u128(1);
        for outcome in [
            QuarantineReleaseOutcome::AuctionRecoveryReturn {
                obligation_id: Uuid::nil(),
                return_mail_id: valid,
                recovery_id: valid,
                recovery_handoff_id: valid,
            },
            QuarantineReleaseOutcome::AuctionRecoveryReturn {
                obligation_id: valid,
                return_mail_id: Uuid::nil(),
                recovery_id: valid,
                recovery_handoff_id: valid,
            },
            QuarantineReleaseOutcome::AuctionRecoveryReturn {
                obligation_id: valid,
                return_mail_id: valid,
                recovery_id: Uuid::nil(),
                recovery_handoff_id: valid,
            },
            QuarantineReleaseOutcome::AuctionRecoveryReturn {
                obligation_id: valid,
                return_mail_id: valid,
                recovery_id: valid,
                recovery_handoff_id: Uuid::nil(),
            },
        ] {
            assert!(outcome.validate().is_err(), "恢复接管引用为零 UUID 时必须拒绝");
        }
    }

    #[test]
    fn release_hold_request_rejects_nil_hold_and_unknown_fields() {
        let valid = r#"{"command_id":"00000000-0000-4000-8000-000000000001","expected_version":2,"hold_id":"00000000-0000-4000-8000-000000000002"}"#;
        assert!(serde_json::from_str::<ReleaseConsignmentHoldRequest>(valid).is_ok());
        let nil_hold = valid.replace(
            "\"hold_id\":\"00000000-0000-4000-8000-000000000002\"",
            "\"hold_id\":\"00000000-0000-0000-0000-000000000000\"",
        );
        assert!(serde_json::from_str::<ReleaseConsignmentHoldRequest>(&nil_hold).is_err());
        let unknown = valid.replace('}', r#","force":true}"#);
        assert!(serde_json::from_str::<ReleaseConsignmentHoldRequest>(&unknown).is_err());
    }
}

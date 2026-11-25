//! Admin 订单 recheck 与 resolve 的请求与回执。
//!
//! 请求只收固定字段：稳定 UUIDv4 命令 ID、预期合同版本，resolve 另加动作与证据 ID；没有金额、收件人
//! 或装备字段（金额只由 6B 单写者按冻结事实决定，不变量 4、6）。回执只含 ID、版本与动作，
//! 不含买家、卖家或装备 key（不变量 7）。

use uuid::Uuid;

use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

string_enum! {
    /// resolve 的动作闭集（D9 §6.2）；退款类三个动作共用同一个 6B 写端。
    pub enum OrderResolveActionV2 {
        /// 取消未付购买。
        CancelUnpaid => "cancel_unpaid",
        /// 原件退回后全额退款。
        RefundReturned => "refund_returned",
        /// 实物灭失后全额退款。
        RefundLost => "refund_lost",
        /// 换季清空后全额退款。
        RefundSeason => "refund_season",
        /// 保留原受益用途。
        RetainEntitlement => "retain_entitlement",
        /// 补寄原件：只开新轮，上一轮必须已明确封闭。
        RedispatchOriginal => "redispatch_original",
    }
}

string_enum! {
    /// recheck 回执的固定操作名。
    pub enum RecheckOperation {
        /// 排入订单复核。
        Recheck => "recheck",
    }
}

string_enum! {
    /// recheck 回执的固定处置。
    pub enum RecheckDisposition {
        /// 复核工作已排队。
        RecheckQueued => "recheck_queued",
    }
}

string_enum! {
    /// resolve 回执的固定操作名。
    pub enum ResolveOperation {
        /// 订单处置。
        Resolve => "resolve",
    }
}

write_object! {
    /// Admin 排入订单复核：稳定 UUIDv4 与预期合同版本；多余字段按未知字段整体拒绝。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RecheckOrderRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        #[serde(serialize_with = "serialize_positive_version")]
        pub expected_version: i64 => ("positive_version", "必须为正的安全 JSON 整数"),
    }
}

write_object! {
    /// Admin 订单处置：稳定 UUIDv4、预期合同版本、动作与证据 ID；没有金额与收件人字段。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ResolveOrderRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        #[serde(serialize_with = "serialize_positive_version")]
        pub expected_version: i64 => ("positive_version", "必须为正的安全 JSON 整数"),
        pub action: OrderResolveActionV2 => ("serde::Deserialize::deserialize", "必须为闭集动作"),
        pub evidence_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
    }
}

json_object! {
    ()
    /// recheck 的 202 回执：只有合同、版本与复核工作 ID。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RecheckOrderReceiptV2 {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: RecheckOperation,
        pub disposition: RecheckDisposition,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub contract_id: Uuid,
        #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
        pub contract_version: i64,
        #[serde(deserialize_with = "uuid")]
        pub work_id: Uuid,
    } => validate
}

impl RecheckOrderReceiptV2 {
    fn validate(&self) -> Result<(), &'static str> {
        validate_ids(self.schema_version, &[self.command_id, self.contract_id, self.work_id])
    }
}

json_object! {
    (tag = "disposition", rename_all = "snake_case")
    /// resolve 的成功回执：处置已记录（200），或补寄已排队（202，只含将要开的新轮 ID）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ResolveOrderReceiptV2 {
        /// 处置已记录。
        ResolutionRecorded {
            /// 固定 v2。
            schema_version: u16,
            operation: ResolveOperation,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            contract_id: Uuid,
            #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
            contract_version: i64,
            action: OrderResolveActionV2,
            #[serde(deserialize_with = "uuid")]
            resolution_id: Uuid,
        },
        /// 补寄已排队；动作固定为补寄，不再带动作字段。
        RedispatchQueued {
            /// 固定 v2。
            schema_version: u16,
            operation: ResolveOperation,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            contract_id: Uuid,
            #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
            contract_version: i64,
            #[serde(deserialize_with = "uuid")]
            dispatch_id: Uuid,
        },
    } => validate
}

impl ResolveOrderReceiptV2 {
    fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::ResolutionRecorded {
                schema_version,
                command_id,
                contract_id,
                action,
                resolution_id,
                ..
            } => {
                if *action == OrderResolveActionV2::RedispatchOriginal {
                    return Err("补寄只能以 redispatch_queued 回执");
                }
                validate_ids(*schema_version, &[*command_id, *contract_id, *resolution_id])
            }
            Self::RedispatchQueued {
                schema_version, command_id, contract_id, dispatch_id, ..
            } => validate_ids(*schema_version, &[*command_id, *contract_id, *dispatch_id]),
        }
    }
}

fn validate_ids(schema_version: u16, ids: &[Uuid]) -> Result<(), &'static str> {
    if schema_version != 2 {
        return Err("订单命令回执的版本不一致");
    }
    if ids.iter().any(Uuid::is_nil) {
        return Err("订单命令回执的标识不得为零 UUID");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        OrderResolveActionV2, RecheckOrderReceiptV2, RecheckOrderRequest, ResolveOrderReceiptV2,
        ResolveOrderRequest,
    };

    const COMMAND: &str = "00000000-0000-4000-8000-000000000001";
    const CONTRACT: &str = "00000000-0000-4000-8000-000000000002";
    const EVIDENCE: &str = "00000000-0000-4000-8000-000000000003";

    fn resolve_request() -> Value {
        json!({
            "command_id": COMMAND, "expected_version": 3,
            "action": "refund_returned", "evidence_id": EVIDENCE,
        })
    }

    /// 动作是闭集；请求没有金额、收件人或装备字段，多余字段与非 v4 命令 ID 整体拒绝。
    #[test]
    fn requests_accept_only_the_closed_action_set_and_fixed_fields() {
        let ok: ResolveOrderRequest = serde_json::from_value(resolve_request()).expect("合法请求");
        assert_eq!(ok.action, OrderResolveActionV2::RefundReturned);
        for action in [
            "cancel_unpaid",
            "refund_returned",
            "refund_lost",
            "refund_season",
            "retain_entitlement",
            "redispatch_original",
        ] {
            let mut body = resolve_request();
            body["action"] = json!(action);
            assert!(serde_json::from_value::<ResolveOrderRequest>(body).is_ok(), "{action}");
        }
        for action in ["refund", "REFUND_LOST", "refund_all", ""] {
            let mut body = resolve_request();
            body["action"] = json!(action);
            assert!(serde_json::from_value::<ResolveOrderRequest>(body).is_err(), "{action}");
        }
        for extra in ["amount", "recipient", "equipment_url", "refund"] {
            let mut body = resolve_request();
            body[extra] = json!(1);
            assert!(serde_json::from_value::<ResolveOrderRequest>(body).is_err(), "{extra}");
        }
        let mut body = resolve_request();
        body["command_id"] = json!("00000000-0000-1000-8000-000000000001");
        assert!(serde_json::from_value::<ResolveOrderRequest>(body).is_err());
        let recheck = json!({ "command_id": COMMAND, "expected_version": 1 });
        assert!(serde_json::from_value::<RecheckOrderRequest>(recheck.clone()).is_ok());
        let mut extra = recheck;
        extra["action"] = json!("refund_lost");
        assert!(serde_json::from_value::<RecheckOrderRequest>(extra).is_err());
    }

    /// 回执形状：补寄只能 redispatch_queued 且不带动作；处置记录不能是补寄；多余字段拒绝。
    #[test]
    fn receipts_keep_the_two_shapes_apart() {
        let recorded = json!({
            "disposition": "resolution_recorded", "schema_version": 2, "operation": "resolve",
            "command_id": COMMAND, "contract_id": CONTRACT, "contract_version": 4,
            "action": "refund_lost", "resolution_id": EVIDENCE,
        });
        assert!(serde_json::from_value::<ResolveOrderReceiptV2>(recorded.clone()).is_ok());
        let mut wrong = recorded.clone();
        wrong["action"] = json!("redispatch_original");
        assert!(serde_json::from_value::<ResolveOrderReceiptV2>(wrong).is_err());
        let mut money = recorded;
        money["amount"] = json!(1);
        assert!(serde_json::from_value::<ResolveOrderReceiptV2>(money).is_err());
        let queued = json!({
            "disposition": "redispatch_queued", "schema_version": 2, "operation": "resolve",
            "command_id": COMMAND, "contract_id": CONTRACT, "contract_version": 4,
            "dispatch_id": EVIDENCE,
        });
        assert!(serde_json::from_value::<ResolveOrderReceiptV2>(queued.clone()).is_ok());
        let mut with_action = queued;
        with_action["action"] = json!("redispatch_original");
        assert!(serde_json::from_value::<ResolveOrderReceiptV2>(with_action).is_err());
        let recheck = json!({
            "schema_version": 2, "operation": "recheck", "disposition": "recheck_queued",
            "command_id": COMMAND, "contract_id": CONTRACT, "contract_version": 1,
            "work_id": EVIDENCE,
        });
        assert!(serde_json::from_value::<RecheckOrderReceiptV2>(recheck).is_ok());
    }
}

//! 首轮随购买同事务写入：prepared 投递、轮行与冻结身份一次落库。
//!
//! 保护不变量 2、3、5、6：本原语只在调用方的购买事务内运行（命令、买卖双方与实物锁由调用方持有），
//! 不取锁、不提交；首轮键为 `(contract, obligation, 1)`，投递与命令 ID 由轮 ID 确定性派生，
//! 所以重放同一购买得到同一行。投递停在 `prepared`，通用调度器只领取 `pending`，
//! 首次起写必须经 `mark_write_started`。

use auction_application::consignment::ConsignmentError;
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref, dispatch_round_id,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentObligationGraph, settlement_world_code,
};
use auction_domain::{SettlementObligationKind, legacy_natural_key_uuid};
use serde_json::{Value, json};
use sqlx::Postgres;
use uuid::Uuid;

use super::round::{Authorization, insert_round};
use crate::database::TrackedTransaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::delivery::audit::{DeliveryAudit, PREPARE, append as append_delivery_audit};

const DELIVERY_NAMESPACE: &str = "consignment.dispatch-delivery.v1";
const DELIVERY_COMMAND_NAMESPACE: &str = "consignment.dispatch-delivery-command.v1";

/// 首轮写入的结果：轮、投递与被履行的付款义务。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FirstRound {
    /// 轮 ID，即账本目标与邮件模板的独立轮引用。
    pub(crate) round_id: Uuid,
    /// 轮的投递（`prepared`）。
    pub(crate) delivery_id: Uuid,
    /// 被履行的付款义务（COD 原件或付款载体）。
    pub(crate) obligation_id: Uuid,
}

/// 轮的确定性投递 ID 与投递命令 ID；字节随黄金向量冻结。
pub(super) fn round_delivery_ids(round_id: Uuid) -> (Uuid, Uuid) {
    (
        legacy_natural_key_uuid(DELIVERY_NAMESPACE, &[round_id.as_bytes()]),
        legacy_natural_key_uuid(DELIVERY_COMMAND_NAMESPACE, &[round_id.as_bytes()]),
    )
}

/// 小写十六进制；投递载荷里的冻结身份摘要用它表达。
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 轮投递的载荷：只含发送所需的冻结事实；身份摘要让起写前后能核对“同一封”。
/// 首轮与补寄新轮共用同一形状（补寄另加 `frozen` 段，见 `redispatch`）。
pub(super) fn send_payload(frozen: &FrozenDispatchIdentity) -> Value {
    json!({
        "operation": "send",
        "world": settlement_world_code(frozen.world),
        "template": frozen.template,
        "subject": frozen.subject,
        "credits": frozen.credits,
        "cod": frozen.cod,
        "equipment_ids": frozen.equipment_ids,
        "round_ref": frozen.round_ref,
        "identity_digest": hex(&frozen.identity_digest()),
    })
}

/// 载荷里的附件装备清单；缺失或形状不对返回 `None`（调用方失败关闭）。
pub(super) fn payload_equipment_ids(payload: &Value) -> Option<Vec<String>> {
    payload
        .get("equipment_ids")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// 载荷冻结的身份摘要是否就是该冻结身份的摘要。
pub(super) fn payload_matches(payload: &Value, frozen: &FrozenDispatchIdentity) -> bool {
    payload.get("identity_digest").and_then(Value::as_str)
        == Some(hex(&frozen.identity_digest()).as_str())
}

/// 在购买事务内建立首轮。调用方须已在同一事务写入合同义务图（`write_graph_in_conn`）。
///
/// 首轮冻结的权利人必须是合同冻结的买方（本轮行使买方交割权，收件受益人即买方）。
/// 金额与来源核对失败时整笔回滚，不留半个首轮：付款义务必须是图的首项且无前置，冻结的世界、
/// Credits 与 COD 必须等于义务金额加载体本金（main 为 P，isekai 载体为 P+1）。
pub(crate) async fn create_first_round_in_tx(
    tx: &mut TrackedTransaction<'_, Postgres>,
    graph: &ConsignmentObligationGraph,
    frozen: &FrozenDispatchIdentity,
) -> Result<FirstRound, WriteError> {
    frozen.validate().map_err(invalid_request)?;
    let ConsignmentContract::Sale { core, terms } = &graph.contract else {
        return Err(ConsignmentError::InvalidDepositRequest.into());
    };
    let Some(payment) = graph.obligations.first() else {
        return Err(ConsignmentError::InvalidDepositRequest.into());
    };
    let first_leg = matches!(
        payment.kind,
        SettlementObligationKind::WinnerEquipmentCod
            | SettlementObligationKind::WinnerPaymentCarrierCod
    );
    let expected_cod = payment.amount.value().checked_add(i64::from(payment.carrier_credit));
    if !first_leg
        || payment.prerequisite_id.is_some()
        || payment.contract_id != core.contract_id
        || frozen.rights_holder_identity != terms.frozen_buyer_identity
        || frozen.credits != i64::from(payment.carrier_credit)
        || Some(frozen.cod) != expected_cod
    {
        return Err(ConsignmentError::InvalidDepositRequest.into());
    }
    let key = DispatchRoundKey {
        contract_id: core.contract_id.as_uuid(),
        obligation_id: payment.id.as_uuid(),
        round: 1,
    };
    if frozen.round_ref != dispatch_ref(&key) {
        return Err(ConsignmentError::InvalidDepositRequest.into());
    }
    insert_first_round(tx, key, frozen, &terms.frozen_buyer_identity).await
}

/// 首轮的共用写入：核对履约行的派发世界，写 `prepared` 投递（收件人即冻结权利人）、轮行与准备审计。
///
/// 调用方已按合同种类核过义务、金额与冻结身份；本函数只落库，不取锁、不提交（不变量 3：与合同同事务）。
pub(super) async fn insert_first_round(
    tx: &mut TrackedTransaction<'_, Postgres>,
    key: DispatchRoundKey,
    frozen: &FrozenDispatchIdentity,
    recipient: &str,
) -> Result<FirstRound, WriteError> {
    let round_id = dispatch_round_id(&key);
    let (delivery_id, delivery_command_id) = round_delivery_ids(round_id);
    let identity_digest = frozen.identity_digest();

    // 派发世界在履约行里已由义务形状决定；冻结身份与它不一致就失败关闭。
    let dispatch_world: String = sqlx::query_scalar(
        "SELECT dispatch_world FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(key.obligation_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| WriteError::invariant("consignment_dispatch_fulfillment_missing"))?;
    if dispatch_world != settlement_world_code(frozen.world) {
        return Err(ConsignmentError::InvalidDepositRequest.into());
    }

    let payload = send_payload(frozen);
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, $3, 'settlement', $4, $5, $6, 'prepared')",
    )
    .bind(delivery_id)
    .bind(delivery_command_id)
    .bind(identity_digest.as_slice())
    .bind(key.obligation_id.to_string())
    .bind(recipient)
    .bind(&payload)
    .execute(&mut **tx)
    .await?;

    // 履约行保持初始 pending 且不指向投递：`c5_contract_graph` 在提交时要求图内每条履约行仍是 pending。
    // 投递与履约行的衔接（既有 prepare 对同 ID、同摘要、同载荷的投递按 ON CONFLICT 采纳）归 executor 集成。
    insert_round(tx, &key, round_id, delivery_id, frozen, Authorization::first()).await?;

    let occurred_at = database_now(tx).await?;
    append_delivery_audit(
        tx,
        DeliveryAudit {
            occurred_at,
            actor_type: "system",
            actor_id: "consignment-dispatch",
            action: PREPARE,
            delivery_id,
            request_id: None,
            source: "system",
            metadata: json!({ "origin_kind": "settlement", "status": "prepared" }),
        },
    )
    .await?;
    Ok(FirstRound { round_id, delivery_id, obligation_id: key.obligation_id })
}

fn invalid_request(error: auction_domain::consignment::dispatch::DispatchRuleError) -> WriteError {
    tracing::warn!(%error, "寄售首轮冻结身份无效");
    ConsignmentError::InvalidDepositRequest.into()
}

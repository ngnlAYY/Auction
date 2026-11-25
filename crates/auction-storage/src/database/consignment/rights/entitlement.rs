//! `EntitlementRecipient`：以冻结受益人核对寄售义务的收件人资格，替代“必须是可参加新交易的用户”。
//!
//! 保护不变量 2、4、6：只读判定，不改任何行。授权要求同时满足——义务确属该合同；义务的冻结受益人
//! UUID 与冻结身份等于合同冻结的买方 / 卖方；该合同该角色的权利行存在、受益人一致且仍 `active`；
//! 若义务已有派发轮，轮上冻结的权利人身份与之一致；受益人当前的游戏身份仍等于冻结身份（身份连续性）。
//! 黑名单状态不是条件：已成交的订单继续履约，被拉黑只阻断新购。
//!
//! 身份连续性只认“当前用户名与冻结身份逐字相同”。改名后冻结名可能已属于别人，站内改名记录不能
//! 证明游戏内同一角色，所以改名一律判未证：保留权利、不授权任何发送，转人工核对（失败关闭）。

use auction_domain::consignment::rights::{RightRole, right_id};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

/// 一次授权请求：义务与调用方持有的冻结收件人（认领时读出的受益人 UUID 与收件名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitlementRequest {
    /// 要履约的义务。
    pub obligation_id: Uuid,
    /// 认领 / 派发里冻结的受益用户；卖款等只带游戏身份的义务没有它，此时只核冻结身份文本。
    pub beneficiary_user_id: Option<Uuid>,
    /// 认领 / 派发里冻结的收件游戏身份。
    pub frozen_recipient: String,
}

/// 保留权利、不予授权的原因（只含固定分类，不含任何身份文本）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainReason {
    /// 合同没有该角色的权利行。
    RightMissing,
    /// 受益用户或冻结身份与合同 / 权利 / 义务不一致。
    BeneficiaryMismatch,
    /// 权利已不是 `active`。
    RightNotActive,
    /// 已有派发轮的冻结权利人身份与受益人不一致。
    DispatchMismatch,
    /// 当前身份与冻结身份不同或无法确认：身份连续性未证。
    IdentityContinuityUnproven,
}

/// 授权结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementDecision {
    /// 义务不属于寄售合同，本规则不适用，沿用调用方原有的收件人资格。
    NotConsignment,
    /// 冻结受益人有效且身份连续，可以继续准备、激活或发送。
    Authorized,
    /// 保留权利、零发送，转核对。
    Retain(RetainReason),
}

/// 义务种类对应的权利角色；非寄售合同义务种类为 `None`。
fn role_for(kind: &str) -> Option<RightRole> {
    match kind {
        "winner_equipment_cod"
        | "winner_payment_carrier_cod"
        | "winner_equipment_handoff"
        | "buyer_refund" => Some(RightRole::Buyer),
        "seller_payout" | "withdrawn_return" | "unsold_return" => Some(RightRole::Seller),
        _ => None,
    }
}

/// 在调用方事务里核对；prepare、activate 与最终 Send 共用。
///
/// **锁序前提（结算既定顺序 `users → obligations → actions → deliveries → fulfillments`，见
/// `settlement/fulfillment/internal/locks.rs`）**：调用方必须已在同一事务里先锁受益用户行
/// （`FOR SHARE` 或更强），再锁履约行（`settlement_obligation_fulfillments ... FOR UPDATE`），然后才调用本函数。
/// 函数内对用户行的 `FOR SHARE` 在已持锁时是同事务重入，不会多等；先锁履约行再锁用户会与结算路径形成
/// 死锁环，不得这样接线。本函数其余只读。
///
/// **残余窗口**：判定只在调用方事务内有效。事务提交之后、外部写入发出之前发生的改名不在本判定覆盖范围，
/// 那一段由外写前的持久起写事务（冻结身份摘要核对）与失败后的人工核对承担，本函数不重复也无法覆盖。
pub async fn authorize_entitlement_recipient_in_tx(
    connection: &mut PgConnection,
    request: &EntitlementRequest,
) -> Result<EntitlementDecision, sqlx::Error> {
    use EntitlementDecision::{Authorized, NotConsignment, Retain};
    use RetainReason::{
        BeneficiaryMismatch, DispatchMismatch, IdentityContinuityUnproven, RightMissing,
        RightNotActive,
    };
    let Some(obligation) = sqlx::query(
        "SELECT obligation.contract_id, obligation.obligation_kind, obligation.recipient_user_id,
                contract.seller_user_id, contract.frozen_seller_identity,
                sale.buyer_user_id, sale.frozen_buyer_identity
         FROM public.settlement_obligations AS obligation
         LEFT JOIN public.consignment_contracts AS contract
           ON contract.contract_id = obligation.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = obligation.contract_id
         WHERE obligation.id = $1",
    )
    .bind(request.obligation_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(Retain(BeneficiaryMismatch));
    };
    let Some(contract_id) = obligation.try_get::<Option<Uuid>, _>("contract_id")? else {
        return Ok(NotConsignment);
    };
    let kind: String = obligation.try_get("obligation_kind")?;
    let Some(role) = role_for(&kind) else {
        return Ok(Retain(BeneficiaryMismatch));
    };
    let frozen: Option<(Uuid, String)> = match role {
        RightRole::Buyer => obligation
            .try_get::<Option<Uuid>, _>("buyer_user_id")?
            .zip(obligation.try_get::<Option<String>, _>("frozen_buyer_identity")?),
        RightRole::Seller => obligation
            .try_get::<Option<Uuid>, _>("seller_user_id")?
            .zip(obligation.try_get::<Option<String>, _>("frozen_seller_identity")?),
    };
    let Some((beneficiary, identity)) = frozen else {
        return Ok(Retain(BeneficiaryMismatch));
    };
    let obligation_recipient: Option<Uuid> = obligation.try_get("recipient_user_id")?;
    if request.beneficiary_user_id.is_some_and(|user_id| user_id != beneficiary)
        || request.frozen_recipient != identity
        || obligation_recipient.is_some_and(|recipient| recipient != beneficiary)
    {
        return Ok(Retain(BeneficiaryMismatch));
    }
    let Some(right) = sqlx::query(
        "SELECT right_row.beneficiary_user_id, right_row.frozen_identity, state.state
         FROM public.consignment_rights AS right_row
         JOIN LATERAL (
             SELECT state FROM public.consignment_right_states
             WHERE right_id = right_row.right_id ORDER BY version DESC LIMIT 1
         ) AS state ON true
         WHERE right_row.right_id = $1 AND right_row.contract_id = $2",
    )
    .bind(right_id(contract_id, role))
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(Retain(RightMissing));
    };
    if right.try_get::<Uuid, _>("beneficiary_user_id")? != beneficiary
        || right.try_get::<String, _>("frozen_identity")? != identity
    {
        return Ok(Retain(BeneficiaryMismatch));
    }
    if right.try_get::<String, _>("state")? != "active" {
        return Ok(Retain(RightNotActive));
    }
    let round_identity: Option<String> = sqlx::query_scalar(
        "SELECT rights_holder_identity FROM public.consignment_dispatch_rounds
         WHERE contract_id = $1 AND obligation_id = $2 ORDER BY round DESC LIMIT 1",
    )
    .bind(contract_id)
    .bind(request.obligation_id)
    .fetch_optional(&mut *connection)
    .await?;
    if round_identity.is_some_and(|round_identity| round_identity != identity) {
        return Ok(Retain(DispatchMismatch));
    }
    let current: Option<String> =
        // 不变量 2：对受益用户行加 FOR SHARE——并发改名事务未提交时本判定等待它结束，
        // 提交后读到新名字判未证，不会读到旧值后放行；锁随调用方事务持有到提交。
        sqlx::query_scalar("SELECT username FROM public.users WHERE id = $1 FOR SHARE")
            .bind(beneficiary)
            .fetch_optional(&mut *connection)
            .await?;
    if current.as_deref() != Some(identity.as_str()) {
        return Ok(Retain(IdentityContinuityUnproven));
    }
    Ok(Authorized)
}
